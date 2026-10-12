import { runDurableObjectAlarm } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { age, bringUp, env, call, heartbeat, join, launched, registry, ticket, webhook } from "./helpers";

const V = "0.5.0";
const S = 1000;

async function session(serverId: string) {
	return env.DB.prepare("SELECT * FROM sessions WHERE server_id = ?").bind(serverId).first<Record<string, unknown>>();
}

describe("heartbeats", () => {
	it("overwrite the full record", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0, 4);
		expect((await heartbeat(s.id, s.secret, { players: 2, capacity: 8, state: "open" })).status).toBe(204);
		const [row] = await registry(V).snapshot();
		expect(row).toMatchObject({ state: "ready", players: 2, capacity: 8, server_state: "open", peak_players: 4 });
	});

	it("for an unknown or ended server answer 410", async () => {
		const unknown = await heartbeat("MC41LjA.0123456789abcdef0123456789abcdef", "x", { players: 0, capacity: 10, state: "open" });
		expect(unknown.status).toBe(410);
		await unknown.text();
		const malformed = await heartbeat("nonsense", "x", { players: 0, capacity: 10, state: "open" });
		expect(malformed.status).toBe(410);
		await malformed.text();

		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0);
		expect((await call("POST", `/v1/servers/${s.id}/end`, { reason: "empty" }, { authorization: `Bearer ${s.secret}` })).status).toBe(204);
		const after = await heartbeat(s.id, s.secret, { players: 0, capacity: 10, state: "open" });
		expect(after.status).toBe(410);
		await after.text();
	});

	it("with the wrong secret are refused and change nothing", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		await join({ version: V, locations: ["London"], pings: {} });
		const a = await bringUp(0, 1);
		const b = await bringUp(1, 1);
		const res = await heartbeat(a.id, b.secret, { players: 9, capacity: 10, state: "open" });
		expect(res.status).toBe(401);
		await res.text();
		expect((await registry(V).snapshot()).find((s) => s.id === a.id)?.players).toBe(1);
	});

	it("are validated", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = launched();
		const res = await heartbeat(s.id, s.secret, { players: -1, capacity: 10, state: "open" });
		expect(res.status).toBe(400);
		await res.text();
	});

	it("before the ready webhook fetch the address themselves", async () => {
		const t = await join({ version: V, locations: ["Paris"], pings: {} });
		const s = launched();
		edgegap.ready(edgegap.deployment(0));
		// No webhook: the heartbeat proves the game serves and pulls the address from status.
		expect((await heartbeat(s.id, s.secret, { players: 0, capacity: 10, state: "open" })).status).toBe(204);
		expect((await ticket(t)).state).toBe("ready");
	});
});

describe("readiness", () => {
	it("needs both the Edgegap address and a heartbeat", async () => {
		const t = await join({ version: V, locations: ["Paris"], pings: {} });
		const s = launched();
		edgegap.ready(edgegap.deployment(0));
		expect((await webhook(s.id)).status).toBe(204);
		expect(await ticket(t)).toEqual({ state: "starting" });
		await heartbeat(s.id, s.secret, { players: 0, capacity: 10, state: "open" });
		expect((await ticket(t)).state).toBe("ready");
	});

	it("webhooks are hints: a webhook while Edgegap still deploys changes nothing", async () => {
		const t = await join({ version: V, locations: ["Paris"], pings: {} });
		const s = launched();
		expect((await webhook(s.id)).status).toBe(204);
		expect(edgegap.count("GET", "/v1/status/")).toBe(1);
		expect(await ticket(t)).toEqual({ state: "starting" });
	});

	it("a webhook with the wrong token is refused before any status call", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const res = await webhook(launched().id, "wrong");
		expect(res.status).toBe(401);
		await res.text();
		expect(edgegap.count("GET", "/v1/status/")).toBe(0);
	});

	it("an error webhook fails the server and re-places its tickets elsewhere", async () => {
		const t = await join({ version: V, locations: ["Frankfurt", "Paris"], pings: { Frankfurt: 10, Paris: 20 } });
		const fra = launched(0);
		edgegap.deployment(0).status = "Status.ERROR";
		expect((await webhook(fra.id)).status).toBe(204);
		expect(edgegap.deployment(1).body.filters).toEqual([{ field: "city", values: ["Paris"], filter_type: "any" }]);
		expect(await ticket(t)).toEqual({ state: "starting" });
		const par = await bringUp(1);
		expect((await ticket(t)).key).toBe(par.key);
		expect(await session(fra.id)).toMatchObject({ outcome: "failed", location: "Frankfurt" });
	});

	it("the alarm polls status when the ready webhook is lost", async () => {
		const t = await join({ version: V, locations: ["Paris"], pings: {} });
		const s = launched();
		await heartbeat(s.id, s.secret, { players: 0, capacity: 10, state: "open" });
		edgegap.ready(edgegap.deployment(0));
		await age(V, 6 * S);
		expect(await runDurableObjectAlarm(registry(V))).toBe(true);
		expect((await ticket(t)).state).toBe("ready");
	});
});

describe("the alarm", () => {
	it("drops a server that never becomes ready and stops its deployment", async () => {
		const t = await join({ version: V, locations: ["Paris", "London"], pings: { Paris: 20, London: 25 } });
		const s = launched();
		await age(V, 61 * S); // > START_DEADLINE_S = 60
		expect(await runDurableObjectAlarm(registry(V))).toBe(true);
		expect(edgegap.deployment(0).status).toBe("Status.TERMINATED");
		expect((await registry(V).snapshot())[0]).toMatchObject({ state: "ended", outcome: "failed" });
		expect(await session(s.id)).toMatchObject({ outcome: "failed", reason: "the server did not become ready in time" });
		// The ticket gets one more placement (JOIN_ATTEMPTS = 2), away from the location that failed.
		expect(edgegap.deployment(1).body.filters).toEqual([{ field: "city", values: ["London"], filter_type: "any" }]);
		expect(await ticket(t)).toEqual({ state: "starting" });
	});

	it("fails the ticket when no selected location is left to try", async () => {
		const t = await join({ version: V, locations: ["Paris"], pings: {} });
		await age(V, 61 * S);
		await runDurableObjectAlarm(registry(V));
		expect(edgegap.deployments.size).toBe(1);
		expect(await ticket(t)).toEqual({ state: "failed", reason: "the server did not become ready in time" });
	});

	it("drops a silent server, stops it, and records it lost", async () => {
		const t = await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0, 3);
		await age(V, 10 * S);
		await runDurableObjectAlarm(registry(V));
		expect((await registry(V).snapshot())[0].state).toBe("ready");

		await age(V, 6 * S); // 16 s since the last heartbeat > HEARTBEAT_DEADLINE_S = 15
		await runDurableObjectAlarm(registry(V));
		expect(edgegap.deployment(0).status).toBe("Status.TERMINATED");
		expect((await ticket(t)).state).toBe("failed");
		const row = await session(s.id);
		expect(row).toMatchObject({ outcome: "lost", peak_players: 3 });
		expect(row?.minutes).toBeGreaterThan(0);
		const gone = await heartbeat(s.id, s.secret, { players: 3, capacity: 10, state: "open" });
		expect(gone.status).toBe(410);
		await gone.text();
	});

	it("forgets ended servers after the retention and goes idle", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0);
		await call("POST", `/v1/servers/${s.id}/end`, {}, { authorization: `Bearer ${s.secret}` });
		await age(V, 601 * S);
		await runDurableObjectAlarm(registry(V));
		expect(await registry(V).snapshot()).toEqual([]);
		expect(await runDurableObjectAlarm(registry(V))).toBe(false);
	});
});

describe("end", () => {
	it("stops the deployment and records the session", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0, 2);
		expect((await call("POST", `/v1/servers/${s.id}/end`, { reason: "empty" }, { authorization: `Bearer ${s.secret}` })).status).toBe(204);
		expect(edgegap.count("DELETE", `/v1/stop/${s.deploymentId}`)).toBe(1);
		expect(await session(s.id)).toMatchObject({
			version: V,
			location: "Paris",
			deployment_id: s.deploymentId,
			outcome: "ended",
			reason: "server: empty",
			peak_players: 2,
		});
		expect((await session(s.id))?.ready_at).not.toBeNull();
	});

	it("needs the report secret", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0);
		const res = await call("POST", `/v1/servers/${s.id}/end`, {});
		expect(res.status).toBe(401);
		await res.text();
		expect(edgegap.count("DELETE", "/v1/stop/")).toBe(0);
	});
});
