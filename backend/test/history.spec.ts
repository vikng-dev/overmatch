import { abortAllDurableObjects, runInDurableObject } from "cloudflare:test";
import { describe, expect, it, vi } from "vitest";
import productionServer from "../production-server.json";
import { env, join, launched, registry, webhook } from "./helpers";

const V = "0.5.0";

describe("session allocation history", () => {
	it("records allocation before the provider answers, for the admin's cost estimate", async () => {
		const release = edgegap.holdDeploys();
		const joining = join({ version: V, locations: ["Paris"], pings: {} });
		try {
			await vi.waitFor(() => expect(edgegap.count("POST", "/v1/deploy")).toBe(1));
			expect(await env.DB.prepare("SELECT vcpu, deployment_id FROM sessions").first())
				.toEqual({ vcpu: productionServer.vcpu, deployment_id: null });
		} finally {
			release();
			await joining;
		}
	});

	for (const allocation of [1.25, null]) {
		it(`repairs lost history after eviction using the claim's ${allocation === null ? "unrecorded" : "original"} allocation`, async () => {
			edgegap.loseDeployResponses = 1;
			await join({ version: V, locations: ["Paris"], pings: {} });
			const s = launched();
			await runInDurableObject(registry(V), (_, state) => {
				if (allocation === null) {
					// Fixture schema represents a claim created before allocation recording.
					state.storage.sql.exec("ALTER TABLE servers DROP COLUMN vcpu");
				} else {
					// Fixture allocation differs from today's config; reconciliation must retain it.
					state.storage.sql.exec("UPDATE servers SET vcpu = ? WHERE id = ?", allocation, s.id);
				}
			});
			await env.DB.prepare("DELETE FROM sessions WHERE server_id = ?").bind(s.id).run();
			await abortAllDurableObjects();
			expect((await webhook(s.id, s.deploymentId)).status).toBe(204);
			expect(await env.DB.prepare("SELECT vcpu, deployment_id FROM sessions WHERE server_id = ?").bind(s.id).first())
				.toEqual({ vcpu: allocation, deployment_id: s.deploymentId });
		});
	}
});
