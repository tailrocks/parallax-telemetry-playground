import { tanstackStart } from "@tanstack/react-start/plugin/vite";
import { defineConfig } from "vite";
import viteReact from "@vitejs/plugin-react";
import { sentryVitePlugin } from "@sentry/vite-plugin";

// TanStack Start app. `tanstackStart` provides file-based routing + the server
// (server routes like /v1/traces run on the Nitro server). Sentry source-map
// upload (Debug IDs) is gated on an auth token so a token-less build still works.
const sentryAuthToken = process.env["SENTRY_AUTH_TOKEN"];
const sentryOrg = process.env["SENTRY_ORG"];
const sentryProject = process.env["SENTRY_PROJECT"];

export default defineConfig({
  server: { port: 5173 },
  build: { sourcemap: "hidden" },
  plugins: [
    tanstackStart({ srcDirectory: "src" }),
    viteReact(),
    ...(sentryAuthToken !== undefined &&
    sentryOrg !== undefined &&
    sentryProject !== undefined
      ? [
          sentryVitePlugin({
            org: sentryOrg,
            project: sentryProject,
            authToken: sentryAuthToken,
            // Debug IDs: inject + upload source maps (not the legacy release+dist flow).
            sourcemaps: { filesToDeleteAfterUpload: ["**/*.map"] },
          }),
        ]
      : []),
  ],
});
