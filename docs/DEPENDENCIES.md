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

ESLint 10.15.0 (MIT) lints JavaScript. Oxlint 1.94.0 (MIT) lints TypeScript
and TSX with an explicit rule set and a separate TypeScript compiler check.
The lint policy tests exercise each configured rule and check that generated
files and the design specimen are excluded. jsdom 31.0.0 (MIT) supplies the
DOM for frontend unit tests under the pinned Node version.

The private password credential module uses RustCrypto Argon2 0.6.0 and
zeroize 1.9.1 under MIT or Apache-2.0, and unicode-normalization 0.1.26 under
(MIT or Apache-2.0) and Unicode-3.0. Argon2 runs locally with explicit parameters
and an operating-system random salt. The latter two versions were already
locked. The normal Rust advisory and licence gates cover all three and their
transitive dependencies.

Transitive metadata also includes MIT-0, BSD-2/3-Clause, ISC, 0BSD, CC0-1.0,
CC-BY-4.0, BlueOak-1.0.0, Python-2.0, Unicode-3.0, Zlib and OpenSSL.
The staged web runtime excludes Sharp's native packages because catalogue images
are served as verified renditions. Next's default image optimizer is disabled.
Confirm the absence in the built OCI image and verify the catalogue image route.

Retain attribution/notices and any source obligations when packaging. MPL applies
to covered files; changes to those files require renewed review. No such vendored
modifications are made here. Unknown licences or sources fail the source audit.
The npm inventory covers all locked platforms, including optional uninstalled
binaries; a release still requires an image SBOM and complete distribution notices.

Upgrade policy remains weekly reviewed updates and prompt security fixes. P1 has
no production deployment; a green metadata allowlist alone does not establish
compliance of a future distributed image.

The lint and test updates keep TypeScript on 5.9.3 and Node declarations on
the Node 24 line. Globals 17.14.0 supplies the existing JavaScript globals
under the MIT licence. Ruff 0.17.4 and coverage.py 7.17.2 remain
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

## Frontend test runner and coverage

The TypeScript test suite uses the pinned Node test runner with tsx 4.23.15
for TypeScript and React JSX. The coverage transformer uses the exact esbuild
0.28.2 dependency. Chai 6.3.0 and the standalone `@vitest/expect` and
`@vitest/spy` packages at 5.1.0 retain the matchers and spies. jsdom provides the
DOM. Monocart Coverage Reports 3.2.1 converts V8 coverage into source-mapped
LCOV, including maintained files that no test loaded. The existing aggregate,
per-package and critical-file thresholds still apply. The added packages use
MIT licences and are development dependencies. The DOM declarations come from
`@types/jsdom` 30.0.0 under MIT.

Oxlint replaces the former TypeScript-eslint chain. The Node runner replaces
Vitest and Vite. Both changes were needed to remove picomatch from the locked
dependency tree. The required npm audit still checks all dependencies against
the high threshold. [GitHub advisory](https://github.com/advisories/GHSA-fc5q-39xc-vjpf)

## Development minimatch advisory

ESLint 10.15.0 and its `@eslint/config-array` 0.23.6 dependency select
`minimatch` 11.0.8. This release fixes both
[GHSA-g3hv-75wg-m249](https://github.com/isaacs/minimatch/security/advisories/GHSA-g3hv-75wg-m249)
and [GHSA-c859-jr93-m5j4](https://github.com/isaacs/minimatch/security/advisories/GHSA-c859-jr93-m5j4).
The upstream fix replaces the former local mutation of minimatch 10.2.6.

The TypeScript lint toolchain no longer installs `@typescript-eslint/typescript-estree`
or its former minimatch override. The JavaScript lint toolchain retains positive
and negative controls for its installed `minimatch` interfaces. Minimatch and its
`@isaacs/nonrecursive-glob-match` helper use the reviewed BlueOak-1.0.0 licence.
The lock retains package integrity hashes. Release still requires fresh dependency
and image review and distribution notices.

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
