import path from "node:path";
import { cloudflareTest, readD1Migrations } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

export function workerTests(proving = false) {
	return defineConfig({
	plugins: [
		cloudflareTest(async () => ({
			wrangler: { configPath: "./wrangler.jsonc", environment: proving ? "aws-proving" : undefined },
			miniflare: {
				bindings: {
					...(proving ? { AWS_ACCESS_KEY_ID: "fixture-access", AWS_SECRET_ACCESS_KEY: "fixture-secret", AWS_HOST_CONFIG: JSON.stringify({securityGroupId: "sg-00000000000000001", subnetIds: ["subnet-00000000000000001", "subnet-00000000000000002"]}) } : {}),
					TEST_MIGRATIONS: await readD1Migrations(path.join(import.meta.dirname, "migrations")),
					EDGEGAP_API: "https://edgegap.test",
					EDGEGAP_TOKEN: "fixture-opaque-value",
					PROVIDER_TIMEOUT_S: "0.5",
					ACCESS_TEAM_DOMAIN: "https://team.access.test",
					ACCESS_AUD: "test-aud",
				},
			},
		})),
	],
	test: {
		include: proving ? ["test/ec2.spec.ts"] : ["test/**/*.spec.ts"],
		exclude: proving ? [] : ["test/ec2.spec.ts"],
		setupFiles: ["./test/setup.ts"],
		// The Edgegap fake is reached through the global `fetch` stub (test/setup.ts); files running
		// at once would swap each other's fake mid-test.
		fileParallelism: false,
	},
	});
}
export default workerTests();
