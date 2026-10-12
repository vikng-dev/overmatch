import { runDurableObjectAlarm } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { age, heartbeat, join, launched, registry, ticket, webhook } from "./helpers";

const V = "0.5.0";
const S = 1000;
const PARIS = [{ field: "city", values: ["Paris"], filter_type: "any" }];
const LONDON = [{ field: "city", values: ["London"], filter_type: "any" }];

describe("a deploy whose outcome is unknown", () => {
	it("starts no replacement; the alarm finds the deployment by tag and adopts it", async () => {
		edgegap.loseDeployResponses = 1;
		const t = await join({ version: V, locations: ["Paris", "London"], pings: { Paris: 10, London: 20 } });
		expect(edgegap.deployments.size).toBe(1); // Paris exists; no London started for the same claim
		expect(await ticket(t)).toEqual({ state: "starting" });

		await age(V, 6 * S);
		await runDurableObjectAlarm(registry(V)); // reconciles by tag
		const s = launched(0);
		expect((await registry(V).snapshot())[0].deployment_id).toBe(s.deploymentId);

		edgegap.ready(edgegap.deployment(0));
		await heartbeat(s.id, s.secret, { players: 0, capacity: 10, state: "open" });
		expect(await ticket(t)).toMatchObject({ state: "ready", key: s.key });
		expect(edgegap.deployments.size).toBe(1);
	});

	it("the ready webhook's request id is adopted once its tags prove it ours", async () => {
		edgegap.loseDeployResponses = 1;
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = launched(0);
		await webhook(s.id, undefined, "dep-not-ours"); // unknown id: ignored
		expect((await registry(V).snapshot())[0].deployment_id).toBeNull();
		await webhook(s.id, undefined, s.deploymentId);
		expect((await registry(V).snapshot())[0].deployment_id).toBe(s.deploymentId);
	});

	it("is re-placed only after the deadline, once a lookup confirms nothing was created", async () => {
		edgegap.failDeploysUnanswered = 1;
		const t = await join({ version: V, locations: ["Paris", "London"], pings: { Paris: 10, London: 20 } });
		expect(edgegap.deployments.size).toBe(0);
		await age(V, 6 * S);
		await runDurableObjectAlarm(registry(V)); // lookup: nothing yet; still within the deadline
		expect(edgegap.deployments.size).toBe(0);
		expect(await ticket(t)).toEqual({ state: "starting" });

		await age(V, 60 * S);
		await runDurableObjectAlarm(registry(V)); // past the deadline, absence confirmed: retry elsewhere
		expect(edgegap.deployments.size).toBe(1);
		expect(edgegap.deployment(0).body.filters).toEqual(LONDON);
		expect(await ticket(t)).toEqual({ state: "starting" });
	});

	it("a claim that times out with its deployment found late stops that deployment", async () => {
		edgegap.loseDeployResponses = 1;
		edgegap.hangStatus.add("dep1");
		const t = await join({ version: V, locations: ["Paris", "London"], pings: { Paris: 10, London: 20 } });
		expect(edgegap.deployment(0).body.filters).toEqual(PARIS);
		await age(V, 61 * S);
		await runDurableObjectAlarm(registry(V)); // deadline: the Paris deployment is found and stopped
		expect(edgegap.deployment(0).status).toBe("Status.TERMINATED");
		expect(edgegap.deployment(1).body.filters).toEqual(LONDON);
		expect(await ticket(t)).toEqual({ state: "starting" });
	});
});
