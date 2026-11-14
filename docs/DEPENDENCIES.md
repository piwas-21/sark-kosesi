# Dependency review

P1 uses free libraries and self-hosted runtimes. Exact direct versions and transitive
integrity hashes are in Cargo.lock/package-lock.json; CLI downloads and hashes are
in config/tools.json. No runtime calls a mandatory hosted platform. Provider
contracts remain separate and pending.

| Dependencies                                                                   | Reviewed licence                     | Handling                                                                                                                                                                         |
| ------------------------------------------------------------------------------ | ------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Axum, Tokio, serde, SQLx, tracing, utoipa, tower, subtle, getrandom, sha2      | MIT / Apache-2.0                     | Retain notices; Rust licence/source allowlist enforced by cargo-deny                                                                                                             |
| Next.js 16.4.6, React 19.3.1, Tailwind 4.3.5, openapi-fetch 0.17.0, TS tooling | MIT / Apache-2.0                     | Self-hosted SSR; no Vercel service required                                                                                                                                      |
| TypeScript 5.9.3, @hey-api/openapi-ts 0.99.0                                   | Apache-2.0 / MIT                     | Exact development versions; generator output and the owned openapi-fetch adapter are checked against Rust OpenAPI                                                                |
| Bricolage Grotesque, DM Sans                                                   | OFL-1.1                              | Existing project font sources subset to WOFF; licences retained in public/fonts; no reserved font names declared in supplied notices                                             |
| PostgreSQL                                                                     | PostgreSQL licence                   | Disposable native cluster or digest-pinned official image; no shared database                                                                                                    |
| cargo-llvm-cov, cargo-audit, cargo-deny                                        | MIT / Apache-2.0                     | Checksum-pinned unmodified CLI tools                                                                                                                                             |
| Lighthouse, Playwright, axe                                                    | Apache-2.0 / MPL-2.0                 | Development/test tooling, self-hosted browser runs                                                                                                                               |
| k6 2.3.0                                                                       | AGPL-3.0-only                        | Unmodified standalone test CLI, not linked into the application; retain licence/source reference with redistributed tool                                                         |
| webpki-roots                                                                   | CDLA-Permissive-2.0                  | Trust-root data; preserve the agreement with redistributed data                                                                                                                  |
| sharp/libvips optional native packages                                         | Apache-2.0 / LGPL-3.0-or-later / MIT | Unmodified dynamically loaded libraries. Preserve licence/source/relinking material when distributing runtime images; do not statically combine or modify without renewed review |

ESLint 10.14.0 (MIT) retains the verified minimatch backport, and jsdom 31.0.0
(MIT) runs the frontend unit tests under the pinned Node version.

The private password credential module uses RustCrypto Argon2 0.6.0 and
zeroize 1.9.1 under MIT or Apache-2.0, and unicode-normalization 0.1.26 under
(MIT or Apache-2.0) and Unicode-3.0. Argon2 runs locally with explicit parameters
and an operating-system random salt. The latter two versions were already
locked. The normal Rust advisory and licence gates cover all three and their
transitive dependencies.

Transitive metadata also includes MIT-0, BSD-2/3-Clause, ISC, 0BSD, CC0-1.0,
CC-BY-4.0, BlueOak-1.0.0, Python-2.0, Unicode-3.0, Zlib and OpenSSL.
Retain attribution/notices and any source obligations when packaging. MPL applies
to covered files; changes to those files require renewed review. No such vendored
modifications are made here. Unknown licences or sources fail the source audit.
The npm inventory covers all locked platforms, including optional uninstalled
binaries; a release still requires an image SBOM and complete distribution notices.

Upgrade policy remains weekly reviewed updates and prompt security fixes. P1 has
no production deployment; a green metadata allowlist alone does not establish
compliance of a future distributed image.

The lint and test updates keep TypeScript on 5.9.3 and Node declarations on
the Node 24 line. Globals 17.14.0 and typescript-eslint 8.72.2 keep the existing
MIT licence and minimatch consumers. Ruff 0.17.4 and coverage.py 7.17.2 remain
hash-pinned developer tools. Ruff uses the MIT licence and coverage.py uses
Apache-2.0.

`sha2` 0.10.9 was already transitively locked and is now an exact direct dependency
for hashing opaque 256-bit staff session tokens. Its MIT/Apache-2.0 terms are in the
same reviewed allowlist. This is not password hashing or an MFA implementation;
P4 still owns maintained password/MFA verification and secure session issuance.

The already pinned `serde_json` is promoted from an infrastructure test dependency
to runtime use for closed, validated specification snapshots. It adds no new
version or hosted service; its MIT/Apache-2.0 licence remains in the existing
reviewed Rust allowlist.

`image` 0.25.10 is pinned for native JPEG/PNG/WebP verification and re-encoding with
default features disabled. Its released MIT/Apache-2.0 package and transitive
licences are checked by the existing Rust audit/deny gates. Other existing locked
versions are preserved. Decoding uses the upstream [limit API](https://docs.rs/image/0.25.10/image/struct.Limits.html)
with additional owned resource checks; the library documents allocation limits as
best-effort. This adds no hosted image service. New release-image SBOM/notices and
actual photographic acceptance remain release requirements.

`tower-sessions` 0.15.0 (MIT) supplies maintained Axum cookie/session transport with
its memory store disabled. The owned SQLx store binds immutable guest identity to
a SHA-256 verifier of the library's 128-bit identifier; no raw credential is stored.
`async-trait` 0.1.92 (MIT/Apache-2.0) implements its store interface, and `url` 2.5.8
(MIT/Apache-2.0, already locked transitively) validates an explicit HTTPS origin.
Exact direct versions and the added dependency closure pass cargo-audit/cargo-deny.
The HTTP adapter also normalizes removal-cookie attributes and clears failed
creation state so middleware cannot retry an issuance rejected by the quota.
This is guest identity only; verified accounts, privilege rotation and guest-order
access remain separate work. No password or MFA verification is introduced.

Owned quote snapshot tests add a domain dev-dependency edge to the already pinned
`serde_json` 1.0.154 for closed round-trip/corruption checks. No locked package
version changes; its existing MIT/Apache-2.0 review applies.

`email_address` 0.2.9 (MIT) is an exact domain dependency with default features
disabled and no additional transitive packages. The order boundary uses only its
bounded parse result, disables display names/domain literals and requires a TLD.
UTF-8 mailbox syntax remains supported. Syntax does not prove mailbox ownership,
postal delivery or transport-provider SMTPUTF8 support. Release notices and the
existing cargo-audit/cargo-deny gates apply.

Callback-worker integration tests reuse the already pinned workspace `tracing`
package as an infrastructure dev-dependency. This lets isolated loopback tests
execute the same worker configuration, verified GET orchestration and redacted
reporting code. No package version, production dependency or provider-origin
configuration changes; the existing MIT/Apache-2.0 review applies.

## Development minimatch advisory

`GHSA-g3hv-75wg-m249` is a moderate resource-exhaustion advisory affecting the
locked `minimatch` 10.2.6 copy in ESLint and typescript-eslint. The replacement
OpenAPI generator removes Redocly and its 5.1.9 copy. The exact installed lint
backport below addresses the advisory in the maintained lint entrypoint.
The advisory's fix is 11.0.1, outside the installed parents' declared major ranges.
Current compatible parent metadata still selects those older major ranges.
See the [reviewed advisory](https://github.com/advisories/GHSA-g3hv-75wg-m249).

The prior development-only exception expired at 2026-11-03T00:00:00Z; it does not authorize continued affected work. No replacement exception is recorded.
The lint entrypoint applies a development-only backport of upstream minimatch
[commit 77a0f5c](https://github.com/isaacs/minimatch/commit/77a0f5c4b80bdc641c3c930a1d9edf5ad2db57f8)
to the locked 10.2.6 CommonJS and ESM distributions before loading ESLint. It
checks original and patched SHA-256 digests, package metadata, both file states
and the current lint consumers' CommonJS resolution; unknown bytes, mixed states, symlinks
or unexpected consumers fail the lint gate. An interrupted patch requires a
fresh `npm ci --ignore-scripts`. The normal tooling tests exercise both module
formats, bounded expansion, healthy matching and rejection controls. This alters
only the local installed package and does not change npm's lock or audit result.

The normal npm audit still reports 11 propagated moderate findings against the
registry version. The lock and audit retain those findings. No audit waiver or
forced-major override is used. The maintained lint command rejects additional
locked minimatch copies or consumers and requires the verified backport before
loading ESLint. Re-evaluate the patch and dependency closure when a compatible
upstream release lands. Production release requires a fresh dependency and image
review.

The separate [GHSA-c859-jr93-m5j4](https://github.com/isaacs/minimatch/security/advisories/GHSA-c859-jr93-m5j4)
also affects the locked development copy, `minimatch` 10.2.6: `escape()` can
produce a glob with unintended matching semantics. The backport above addresses
GHSA-g3hv-75wg-m249 and does not fix this `escape()` advisory. Review of the
installed locked consumers found ESLint and `@eslint/config-array` constructing
`Minimatch`, and `@typescript-eslint/typescript-estree` calling `minimatch()`;
no current call to `escape()` was identified in those consumers or the tracked
application and lint paths. Recheck usage and the advisory on dependency, lint
configuration or build-path changes. This is a source and installed-graph review;
the runtime image has not been inspected for this assessment.

The contract generator uses `@hey-api/openapi-ts` 0.99.0 (MIT) for schema and
parameter types. The upstream parser 1.4.4 pins `js-yaml` 4.2.0;
a narrow development-only npm override selects the same-major security release
4.3.2 (MIT). This departs from that parser's exact pin and requires review and
contract regeneration controls when either tool changes. The schema-only output
represents OpenAPI binary strings as `Blob | File`; callers must select an
appropriate fetch response parser. An owned fail-closed adapter retains paths,
methods, request and response media types and body presence for `openapi-fetch`.
Unsupported operation shapes fail generation. Exact Rust OpenAPI, model and adapter
bytes are checked separately; typed contract controls exercise their consumers.
No vulnerability exception is added by this change.

The payment-request application now directly uses the already locked `serde`,
`serde_json` 1.0.154, `sha2` 0.10.9 and `url` 2.5.8 packages. Their existing
MIT/Apache-2.0 reviews apply; no package version or dependency closure changes.
Maintained URL parsing validates explicit merchant endpoints, and SHA-256 binds
canonical configuration/request bytes. A digest alone grants no provider authority.

The private test-provider HTTP boundary pins Reqwest 0.13.5 without default features,
using its supported `rustls-no-provider` feature and an explicit preconfigured
Rustls 0.23.46 Ring client with WebPKI-roots 1.0.11. URL 2.5.8 is now a direct
infrastructure dependency. Rustls/roots/URL versions already existed in the lock;
no existing package/version is removed or upgraded. Reqwest and its transitive
platform/HTTP/WASM support add 39 locked packages, all under the existing reviewed
free licence/source policy. Default charset, HTTP/2, proxy, compression and cookie
features are not selected. No paid SDK or provider engine is introduced.
Hyper 1.12.0, Hyper-util 0.1.21, HTTP-body-util 0.1.5 and Bytes 1.12.1 become exact test
edges over already locked packages, providing a protocol-correct bounded loopback
instrument. They add no provider credential or production service.

Private payment-list decoding enables the `raw_value` feature of the already
locked serde_json 1.0.154 (MIT/Apache-2.0). Borrowed original entry bytes preserve
duplicate-field rejection through selection and full validation. This changes no
package version or dependency closure. Pagination follows the provider's
[list contract](https://docs.mollie.com/reference/list-payments) and
[cursor convention](https://docs.mollie.com/reference/pagination), with stricter
owned request/response budgets and no automatic URL following.

The private reversal observation parser directly pins the already locked `time`
0.3.55 (MIT/Apache-2.0) with its parsing feature for typed RFC 3339 timestamps.
It changes no package version or dependency closure; its licence remains covered
by the Rust source/licence allowlist.
