import type { NextConfig } from "next";
const config: NextConfig = {
  output: "standalone",
  poweredByHeader: false,
  transpilePackages: ["@piwas/design", "@piwas/contracts"],
  experimental: { cpus: 2 },
};
export default config;
