# Dependency review

P1 uses free libraries and self-hosted runtimes. Exact direct versions and transitive
integrity hashes are in Cargo.lock/package-lock.json; CLI downloads and hashes are
in config/tools.json. No runtime calls a mandatory hosted platform. Provider
contracts remain separate and pending.

| Dependencies                                                                   | Reviewed licence                     | Handling                                                                                                                                                                         |
| ------------------------------------------------------------------------------ | ------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Axum, Tokio, serde, SQLx, tracing, utoipa, tower, subtle, getrandom, sha2      | MIT / Apache-2.0                     | Retain notices; Rust licence/source allowlist enforced by cargo-deny                                                                                                             |
| Next.js 16.4.3, React 19.3.1, Tailwind 4.3.3, openapi-fetch 0.17.0, TS tooling | MIT / Apache-2.0                     | Self-hosted SSR; no Vercel service required                                                                                                                                      |
| TypeScript 5.9.3                                                               | Apache-2.0                           | Kept within the declared openapi-typescript 7.13.0 peer range; no forced peer resolution                                                                                         |
| Bricolage Grotesque, DM Sans                                                   | OFL-1.1                              | Existing project font sources subset to WOFF; licences retained in public/fonts; no reserved font names declared in supplied notices                                             |
| PostgreSQL                                                                     | PostgreSQL licence                   | Disposable native cluster or digest-pinned official image; no shared database                                                                                                    |
| cargo-llvm-cov, cargo-audit, cargo-deny                                        | MIT / Apache-2.0                     | Checksum-pinned unmodified CLI tools                                                                                                                                             |
| Lighthouse, Playwright, axe                                                    | Apache-2.0 / MPL-2.0                 | Development/test tooling, self-hosted browser runs                                                                                                                               |
| k6 2.3.0                                                                       | AGPL-3.0-only                        | Unmodified standalone test CLI, not linked into the application; retain licence/source reference with redistributed tool                                                         |
| webpki-roots                                                                   | CDLA-Permissive-2.0                  | Trust-root data; preserve the agreement with redistributed data                                                                                                                  |
| sharp/libvips optional native packages                                         | Apache-2.0 / LGPL-3.0-or-later / MIT | Unmodified dynamically loaded libraries. Preserve licence/source/relinking material when distributing runtime images; do not statically combine or modify without renewed review |

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

`sha2` 0.10.9 was already transitively locked and is now an exact direct dependency
for hashing opaque 256-bit staff session tokens. Its MIT/Apache-2.0 terms are in the
same reviewed allowlist. This is not password hashing or an MFA implementation;
P4 still owns maintained password/MFA verification and secure session issuance.
