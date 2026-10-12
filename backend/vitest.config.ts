import path from "node:path";
import { cloudflareTest, readD1Migrations } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({
	plugins: [
		cloudflareTest(async () => ({
			wrangler: { configPath: "./wrangler.jsonc" },
			miniflare: {
				bindings: {
					TEST_MIGRATIONS: await readD1Migrations(path.join(import.meta.dirname, "migrations")),
					EDGEGAP_API: "https://edgegap.test",
					EDGEGAP_TOKEN: "test-token",
					PROVIDER_TIMEOUT_S: "0.5",
					EDGEGAP_WEBHOOK_SECRET: "test-webhook-secret",
					ACCESS_TEAM_DOMAIN: "https://team.access.test",
					ACCESS_AUD: "test-aud",
				},
			},
		})),
	],
	test: {
		setupFiles: ["./test/setup.ts"],
		// The Edgegap fake is reached through the global `fetch` stub (test/setup.ts); files running
		// at once would swap each other's fake mid-test.
		fileParallelism: false,
	},
});
