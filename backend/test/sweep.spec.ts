import { describe, expect, it, vi } from "vitest";
import { adminPage } from "../src/admin";
import { Registry } from "../src/registry";
import { sweep } from "../src/sweep";
import { bringUp, call, env, expectStatus, join, launched, registry } from "./helpers";

const V = "0.5.0";
const S = 1000;
/** START_DEADLINE_S in wrangler.jsonc: the grace an unmatched tag gets. */
const GRACE = 60 * S;
const UNKNOWN_TAG = "om-0123456789abcdef0123456789abcdef";

function running(id: string): boolean {
	const d = edgegap.deployments.get(id);
	return d?.status === "Status.READY" || d?.status === "Status.DEPLOYING";
}

/** A server that ended normally, whose deployment Edgegap still runs after confirming the stop. */
async function leakedAfterEnd(n: number, location: string): Promise<string> {
	await join({ version: V, locations: [location], pings: {} });
	const s = await bringUp(n);
	expectStatus(await call("POST", `/v1/servers/${s.id}/end`, { reason: "empty" }, { authorization: `Bearer ${s.secret}` }), 204);
	expect((await registry(V).snapshot()).find((r) => r.id === s.id)?.state).toBe("ended");
	edgegap.ready(edgegap.deployment(n));
	return s.deploymentId;
}

describe("the orphan sweep", () => {
	it("stops a deployment whose tag matches no record, once it has stayed unmatched past the grace", async () => {
		const orphan = edgegap.foreign([UNKNOWN_TAG]);
		const other = edgegap.foreign(["someone-else"]);
		const t0 = Date.now();
		expect(await sweep(env, t0)).toMatchObject({ listed: 1, stopped: 0 });
		expect(running(orphan.id)).toBe(true);
		expect(await sweep(env, t0 + GRACE)).toMatchObject({ listed: 1, stopped: 1, failed: 0 });
		expect(running(orphan.id)).toBe(false);
		expect(running(other.id)).toBe(true); // not tagged om-<hex>: not ours
	});

	it("stops a deployment whose record is ended, and one whose history is open but whose registry row is not live", async () => {
		const ended = await leakedAfterEnd(0, "Paris");
		const stale = await leakedAfterEnd(1, "London");
		// The registry ended the row but D1 missed the `ended` write.
		await env.DB.prepare("UPDATE sessions SET ended_at = NULL WHERE deployment_id = ?").bind(stale).run();

		expect(await sweep(env)).toMatchObject({ listed: 2, stopped: 2, failed: 0, error: null });
		expect(running(ended)).toBe(false);
		expect(running(stale)).toBe(false);
	});

	it("leaves a ready server, a starting one and a claim whose deploy id is not yet known alone", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const ready = await bringUp(0);
		await join({ version: V, locations: ["London"], pings: {} });
		const starting = launched(1);
		edgegap.loseDeployResponses = 1;
		await join({ version: V, locations: ["Tokyo"], pings: {} });
		const unknown = launched(2);
		expect((await registry(V).snapshot()).find((r) => r.id === unknown.id)?.deployment_id).toBeNull();

		const later = Date.now() + 10 * GRACE;
		expect(await sweep(env, later)).toMatchObject({ listed: 3, stopped: 0, error: null });
		expect(await sweep(env, later + GRACE)).toMatchObject({ listed: 3, stopped: 0 });
		for (const d of [ready, starting, unknown]) expect(running(d.deploymentId)).toBe(true);
		expect(edgegap.count("DELETE", "/v1/stop/")).toBe(0);
	});

	it("retries a failed stop on the next sweep, with no fresh grace for an unmatched tag", async () => {
		const ended = await leakedAfterEnd(0, "Paris");
		const orphan = edgegap.foreign([UNKNOWN_TAG]);
		const t0 = Date.now();
		edgegap.failStops = 1;
		expect(await sweep(env, t0)).toMatchObject({ stopped: 0, failed: 1, error: "stop: stop HTTP 503" });
		expect(running(ended)).toBe(true);
		expect(await sweep(env, t0 + 1)).toMatchObject({ stopped: 1, failed: 0, error: null });
		expect(running(ended)).toBe(false);

		edgegap.failStops = 1;
		expect(await sweep(env, t0 + GRACE)).toMatchObject({ stopped: 0, failed: 1 });
		expect(running(orphan.id)).toBe(true);
		expect(await sweep(env, t0 + GRACE + 1)).toMatchObject({ stopped: 1, failed: 0 });
		expect(running(orphan.id)).toBe(false);
		expect(await sweep(env, t0 + GRACE + 2)).toMatchObject({ listed: 0, stopped: 0 });
	});

	it("follows the listing's pages", async () => {
		edgegap.maxPageSize = 2;
		const orphans = Array.from({ length: 7 }, (_, i) => edgegap.foreign([`om-${i.toString(16).padStart(32, "0")}`]));
		const t0 = Date.now();
		expect((await sweep(env, t0)).listed).toBe(7);
		expect(edgegap.count("GET", "/v1/deployments?page=")).toBe(4);
		expect(await sweep(env, t0 + GRACE)).toMatchObject({ listed: 7, stopped: 7 });
		for (const d of orphans) expect(running(d.id)).toBe(false);
	});

	it("bounds a stop that never answers, and the other stops still land", async () => {
		const hung = await leakedAfterEnd(0, "Paris");
		const other = await leakedAfterEnd(1, "London");
		edgegap.hangStops.add(hung);
		const started = Date.now();
		expect(await sweep(env)).toMatchObject({ stopped: 1, failed: 1, error: "stop: stop HTTP 0" });
		expect(Date.now() - started).toBeLessThan(5 * S); // PROVIDER_TIMEOUT_S is 0.5 in vitest.config.ts
		expect(running(other)).toBe(false);
		expect(running(hung)).toBe(true);

		const page = await adminPage(env);
		expect(page).toMatch(/2 tagged deployments, 1 stopped, 1 stops failed; last error: stop: stop HTTP 0/);
	});

	it("stops nothing, and waits no longer than the bound, when the registry that owns a claim does not answer", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0);
		const down = vi.spyOn(Registry.prototype, "liveDeployments").mockImplementation(() => new Promise<never>(() => {}) as never);
		expect(await sweep(env)).toMatchObject({ listed: 1, stopped: 0, error: "registry: DeadlineError" });
		down.mockRestore();
		expect(running(s.deploymentId)).toBe(true);
	});
});
