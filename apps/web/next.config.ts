import type { NextConfig } from "next";
const config: NextConfig = {
  output: "standalone",
  poweredByHeader: false,
  transpilePackages: ["@piwas/design", "@piwas/contracts", "@piwas/room-fit"],
  experimental: { cpus: 2 },
};
export default config;
