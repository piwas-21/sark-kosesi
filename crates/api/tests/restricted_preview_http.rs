//! Isolated synthetic role and real loopback API rehearsal, never deployment grants.
#[allow(dead_code, unused_imports)]
mod fixture {
    include!("../../../tests/fixtures/approved_catalogue.rs");
}
use fixture::*;
use sqlx::{ConnectOptions, postgres::PgPoolOptions};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const TOKEN: &str = "synthetic-preview-token-1234567890";

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Server {
    fn stop(&mut self) {
        assert!(
            Command::new("/bin/kill")
                .args(["-s", "INT", &self.0.id().to_string()])
                .status()
                .expect("signal API")
                .success()
        );
        assert!(self.0.wait().expect("stop API").success());
    }
}
fn start(pool: &PgPool) -> (Server, SocketAddr) {
    let reserved = TcpListener::bind("127.0.0.1:0").expect("loopback port");
    let addr = reserved.local_addr().expect("address");
    drop(reserved);
    let mut command = Command::new(env!("CARGO_BIN_EXE_commerce-api"));
    command.env_clear();
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let child = command
        .env("APP_ENV", "staging")
        .env("PREVIEW_DATABASE_PREAPPLIED", "1")
        .env("PREVIEW_TOKEN", TOKEN)
        .env("API_BIND", addr.to_string())
        .env(
            "DATABASE_URL",
            pool.connect_options().to_url_lossy().to_string(),
        )
        .env("PGOPTIONS", "-c search_path=pg_temp,public")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start API");
    (Server(child), addr)
}
fn get_http(addr: SocketAddr, path: &str, authenticated: bool) -> (u16, String, Vec<u8>) {
    http(addr, "GET", path, authenticated)
}
fn http(addr: SocketAddr, method: &str, path: &str, authenticated: bool) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("read timeout");
    let authorization = if authenticated {
        format!("Authorization: Bearer {TOKEN}\r\n")
    } else {
        String::new()
    };
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{authorization}Content-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .expect("request");
    let mut response = Vec::new();
    stream
        .take(262145)
        .read_to_end(&mut response)
        .expect("response");
    assert!(response.len() < 262145, "bounded response");
    let split = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("headers");
    let headers = std::str::from_utf8(&response[..split])
        .expect("header text")
        .to_ascii_lowercase();
    let status = headers
        .lines()
        .next()
        .expect("status")
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse()
        .expect("numeric status");
    (status, headers, response[split + 4..].to_vec())
}
fn ready(server: &mut Server, addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            server.0.try_wait().expect("process status").is_none(),
            "API exited before readiness"
        );
        if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
            assert_eq!(get_http(addr, "/health/ready", false).0, 204);
            return;
        }
        assert!(Instant::now() < deadline, "API did not listen");
        std::thread::sleep(Duration::from_millis(20));
    }
}
async fn ddl(pool: &PgPool, sql: String) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(pool)
        .await
        .expect("synthetic fixture DDL");
}
async fn snapshot(pool: &PgPool) -> Vec<(String, String)> {
    let tables: Vec<String> = sqlx::query_scalar("SELECT pg_catalog.quote_ident(c.relname) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p') ORDER BY c.relname")
        .fetch_all(pool).await.expect("table inventory");
    let mut result = Vec::new();
    for table in tables {
        let query = format!(
            "SELECT coalesce(pg_catalog.jsonb_agg(pg_catalog.to_jsonb(t) ORDER BY pg_catalog.to_jsonb(t)::text),'[]'::jsonb)::text FROM public.{table} t"
        );
        let rows = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
            .fetch_one(pool)
            .await
            .expect("synthetic table snapshot");
        result.push((table, rows));
    }
    result
}
async fn denied(pool: &PgPool, sql: &'static str) {
    let error = sqlx::query(sql)
        .execute(pool)
        .await
        .expect_err("restricted role denied");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("42501")
    );
}

// Remove only the synthetic hash check and immutable guard for this fault,
// then restore the exact original bytes, constraint and guard before comparison.
async fn corrupt_original_and_repair(pool: &PgPool, addr: SocketAddr, product: &str, image: &str) {
    let original: Vec<u8> = sqlx::query_scalar("SELECT bytes FROM public.catalogue_image_originals WHERE asset_id='photo' AND recipe='image-v1'")
        .fetch_one(pool).await.expect("synthetic original");
    let checks: Vec<(String, String)> = sqlx::query_as("SELECT pg_catalog.quote_ident(conname),pg_catalog.pg_get_constraintdef(oid) FROM pg_catalog.pg_constraint WHERE conrelid='public.catalogue_image_originals'::pg_catalog.regclass AND contype='c' AND pg_catalog.pg_get_constraintdef(oid) LIKE '%sha256(bytes)%'")
        .fetch_all(pool).await.expect("hash constraint");
    assert_eq!(checks.len(), 1);
    let (name, definition) = &checks[0];
    ddl(pool, format!("ALTER TABLE public.catalogue_image_originals DROP CONSTRAINT {name}; ALTER TABLE public.catalogue_image_originals DISABLE TRIGGER guard_image_originals; UPDATE public.catalogue_image_originals SET bytes=set_byte(bytes,0,get_byte(bytes,0) # 1) WHERE asset_id='photo' AND recipe='image-v1'; ALTER TABLE public.catalogue_image_originals ENABLE TRIGGER guard_image_originals")).await;
    assert_eq!(get_http(addr, product, true).0, 503);
    assert_eq!(get_http(addr, image, true).0, 503);
    ddl(
        pool,
        "ALTER TABLE public.catalogue_image_originals DISABLE TRIGGER guard_image_originals".into(),
    )
    .await;
    assert_eq!(sqlx::query("UPDATE public.catalogue_image_originals SET bytes=$1 WHERE asset_id='photo' AND recipe='image-v1'").bind(&original).execute(pool).await.expect("restore exact original").rows_affected(), 1);
    ddl(pool, format!("ALTER TABLE public.catalogue_image_originals ENABLE TRIGGER guard_image_originals; ALTER TABLE public.catalogue_image_originals ADD CONSTRAINT {name} {definition}")).await;
    assert_eq!(get_http(addr, product, true).0, 200);
    assert_eq!(get_http(addr, image, true).0, 200);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn preapplied_restricted_role_serves_only_verified_synthetic_preview(pool: PgPool) {
    let _case = media_seed(&pool).await;
    rights(
        &pool,
        1,
        &MediaRightsChange::Grant {
            use_scope: MediaUse::ProductPhotography,
            valid_from: 1100,
            term: RightsTerm::Perpetual,
            permission_ref: "synthetic perpetual permission".into(),
        },
    )
    .await;
    let mut selection = selected();
    selection.image.rights_revision = 2;
    facts_review(
        &pool,
        selection.facts,
        0,
        FactDecision::Approved,
        "facts-approved",
    )
    .await;
    image_review(
        &pool,
        &selection.image,
        0,
        ImageDecision::Approved,
        "image-approved",
    )
    .await;
    copy_draft(&pool, Locale::Nl, 1, None, 0, "copy-nl").await;
    copy_review(
        &pool,
        Locale::Nl,
        1,
        0,
        TranslationDecision::Approved,
        "copy-nl-approved",
    )
    .await;
    let choice = PublicationChoice::Activate { selection };
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("activate", &choice),
    )
    .await
    .expect("synthetic publication");
    pool.execute("INSERT INTO catalogue_translations(sku,locale,title,description,care) VALUES ('DEMO-COVERS','nl','Synthetic cover','Synthetic description','Synthetic care')").await.expect("synthetic legacy translation");

    let suffix = std::process::id();
    let runtime = format!("preview_http_runtime_{suffix}");
    let locks = format!("preview_http_locks_{suffix}");
    let images = format!("preview_http_images_{suffix}");
    ddl(&pool, format!("CREATE ROLE {runtime} LOGIN PASSWORD 'synthetic-only'; CREATE ROLE {locks} NOLOGIN; CREATE ROLE {images} NOLOGIN; REVOKE CREATE ON SCHEMA public FROM PUBLIC; GRANT USAGE ON SCHEMA public TO {runtime},{locks},{images}; GRANT SELECT ON public.product_families,public.physical_components,public.catalogue_items,public.catalogue_variants,public.catalogue_boms,public.catalogue_bom_lines,public.catalogue_definition_events,public.catalogue_factual_reviews,public.catalogue_family_routes,public.catalogue_image_renditions,public.catalogue_image_reviews,public.catalogue_media_assets,public.catalogue_media_rights,public.catalogue_media_serialization,public.catalogue_price_revisions,public.catalogue_publication_revisions,public.catalogue_specification_revisions,public.catalogue_translation_reviews,public.catalogue_translation_revisions,public.stock_balances TO {runtime}; GRANT SELECT(asset_id,recipe,sha256,actor,display_width_px,display_height_px) ON public.catalogue_image_originals TO {runtime}; GRANT SELECT(version,success,checksum) ON public._sqlx_migrations TO {runtime}; GRANT SELECT(sku,locale,title,description,care) ON public.catalogue_translations TO {runtime}; GRANT SELECT(sku),UPDATE(sku) ON public.catalogue_variants TO {locks}; GRANT SELECT(asset_id),UPDATE(asset_id) ON public.catalogue_media_serialization TO {locks}; GRANT SELECT(asset_id,recipe,sha256,bytes) ON public.catalogue_image_originals TO {images}; ALTER FUNCTION public.claim_published_variants(text[]) OWNER TO {locks}; ALTER FUNCTION public.claim_published_assets(text[]) OWNER TO {locks}; ALTER FUNCTION public.preview_original_intact(text) OWNER TO {images}; GRANT EXECUTE ON FUNCTION public.claim_published_variants(text[]),public.claim_published_assets(text[]) TO {runtime}")).await;
    let restricted = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(
            (*pool.connect_options())
                .clone()
                .username(&runtime)
                .password("synthetic-only"),
        )
        .await
        .expect("restricted login");
    let boundary: (bool,bool,bool,bool,bool) = sqlx::query_as("SELECT pg_catalog.has_column_privilege(current_user,'public.catalogue_image_originals','bytes','SELECT'),pg_catalog.has_table_privilege(current_user,'public.catalogue_image_originals','SELECT'),pg_catalog.has_schema_privilege(current_user,'public','CREATE'),pg_catalog.pg_has_role(current_user,$1,'MEMBER'),pg_catalog.pg_has_role(current_user,$2,'MEMBER')")
        .bind(&locks).bind(&images).fetch_one(&restricted).await.expect("effective privilege boundary");
    assert_eq!(boundary, (false, false, false, false, false));
    let current: (String, bool, bool, bool, bool) = sqlx::query_as("SELECT current_user::text,r.rolcanlogin,r.rolsuper,r.rolbypassrls,current_user=pg_catalog.pg_get_userbyid((SELECT relowner FROM pg_catalog.pg_class WHERE oid='public.catalogue_items'::pg_catalog.regclass)) FROM pg_catalog.pg_roles r WHERE r.rolname=current_user")
        .fetch_one(&restricted).await.expect("actual restricted login");
    assert_eq!(current, (runtime.clone(), true, false, false, false));
    let helper_roles: Vec<(bool, bool, bool)> = sqlx::query_as("SELECT rolcanlogin,rolsuper,rolbypassrls FROM pg_catalog.pg_roles WHERE rolname=ANY($1) ORDER BY rolname")
        .bind(vec![locks.clone(), images.clone()]).fetch_all(&pool).await.expect("helper roles");
    assert_eq!(helper_roles, vec![(false, false, false); 2]);
    for sql in [
        "SELECT bytes FROM public.catalogue_image_originals",
        "SELECT * FROM public.catalogue_image_originals",
        "SELECT substring(bytes FROM 1 FOR 1) FROM public.catalogue_image_originals",
        "SELECT o FROM public.catalogue_image_originals o",
        "SELECT * FROM public.customer_sessions",
        "SELECT * FROM public.payment_attempts",
        "UPDATE public.stock_balances SET on_hand=on_hand",
        "UPDATE public._sqlx_migrations SET success=success",
        "DELETE FROM public.commerce_outbox",
        "CREATE TABLE public.preview_forbidden(id int)",
    ] {
        denied(&restricted, sql).await;
    }
    let before = snapshot(&pool).await;
    let (mut server, addr) = start(&restricted);
    ready(&mut server, addr);
    let product_path = "/api/published/nl/products/SET-FILLED";
    assert_eq!(get_http(addr, product_path, false).0, 401);
    assert_eq!(http(addr, "POST", "/api/test/guest/orders", true).0, 404);
    assert_eq!(
        get_http(addr, product_path, true).0,
        503,
        "no helper grant fails closed"
    );
    ddl(
        &pool,
        format!("GRANT EXECUTE ON FUNCTION public.preview_original_intact(text) TO {runtime}"),
    )
    .await;
    let (status, headers, body) = get_http(addr, "/api/catalogue/nl", true);
    assert_eq!(status, 200);
    assert!(headers.contains("cache-control: private, no-store"));
    let catalogue: serde_json::Value = serde_json::from_slice(&body).expect("fixture catalogue");
    assert_eq!(catalogue["production_enabled"], false);
    assert_eq!(catalogue["items"][0]["sku"], "DEMO-COVERS");
    let (status, _, body) = get_http(addr, "/api/published/nl", true);
    assert_eq!(status, 200);
    let page: serde_json::Value = serde_json::from_slice(&body).expect("published page");
    assert_eq!(page["production_enabled"], false);
    let (status, _, body) = get_http(addr, product_path, true);
    assert_eq!(status, 200);
    let product: serde_json::Value = serde_json::from_slice(&body).expect("published product");
    assert_eq!(page["items"][0], product);
    assert_eq!(product["sku"], "SET-FILLED");
    assert_eq!(product["minor_units"], 12345);
    for forbidden in [
        "source_ref",
        "evidence_ref",
        "qualification_ref",
        "actor",
        "asset_id",
        "sha256",
        "on_hand",
        "reserved",
    ] {
        assert!(!product.to_string().contains(forbidden), "private field");
    }
    let path = product["images"][0]["url"].as_str().expect("image URL");
    assert_eq!(path, "/api/published/nl/images/SET-FILLED/1/320");
    assert_eq!(get_http(addr, path, false).0, 401);
    let (status, headers, bytes) = get_http(addr, path, true);
    assert_eq!(status, 200);
    assert!(headers.contains("content-type: image/jpeg"));
    assert!(headers.contains("x-content-type-options: nosniff"));
    let derivative: Vec<u8> = sqlx::query_scalar("SELECT bytes FROM public.catalogue_image_renditions WHERE asset_id='photo' AND recipe='image-v1' AND edge=320").fetch_one(&pool).await.expect("synthetic derivative");
    assert_eq!(bytes, derivative);
    assert_eq!(
        image::load_from_memory(&bytes)
            .expect("decoded derivative")
            .width(),
        32
    );
    assert_eq!(
        get_http(addr, "/api/published/nl/originals/photo", true).0,
        404
    );
    corrupt_original_and_repair(&pool, addr, product_path, path).await;

    server.stop();
    drop(server);
    assert_eq!(
        snapshot(&pool).await,
        before,
        "preview must not write database rows"
    );
    restricted.close().await;
    ddl(&pool, format!("ALTER FUNCTION public.claim_published_variants(text[]) OWNER TO CURRENT_USER; ALTER FUNCTION public.claim_published_assets(text[]) OWNER TO CURRENT_USER; ALTER FUNCTION public.preview_original_intact(text) OWNER TO CURRENT_USER; DROP OWNED BY {runtime},{locks},{images}; DROP ROLE {runtime},{locks},{images}")).await;
}
