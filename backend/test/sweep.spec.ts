import { describe, expect, it, vi } from "vitest";
import { adminPage } from "../src/admin";
import { Registry } from "../src/registry";
import { sweep, SWEEP_MAX_STOPS } from "../src/sweep";
import { bringUp, call, env, expectStatus, join, launched, registry } from "./helpers";

const V = "0.5.0";
const S = 1000;
const MINUTE = 60 * S;
const DAY = 24 * 60 * MINUTE;
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
	it("never stops a tagged deployment whose status names no version, and counts it unattributed", async () => {
		const orphan = edgegap.foreign([UNKNOWN_TAG]);
		const other = edgegap.foreign(["someone-else"]);
		const t0 = Date.now();
		for (const t of [t0, t0 + 10 * MINUTE, t0 + DAY]) {
			expect(await sweep(env, t)).toMatchObject({ listed: 1, unattributed: 1, stopped: 0 });
		}
		expect(running(orphan.id)).toBe(true);
		expect(running(other.id)).toBe(true); // not tagged om-<hex>: not ours
	});

	it("keeps a live server whose status carries no app_version through repeated sweeps", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0);
		edgegap.omitVersion = true;
		const t0 = Date.now();
		for (const t of [t0, t0 + 10 * MINUTE, t0 + DAY]) {
			expect(await sweep(env, t)).toMatchObject({ listed: 1, unattributed: 1, stopped: 0 });
		}
		expect(running(s.deploymentId)).toBe(true);
		expect(await adminPage(env)).toMatch(/1 tagged deployments, 1 unattributed, 0 stopped/);
	});

	it("stops a deployment whose registry row is ended, and one of a known version that no row owns, at once", async () => {
		const ended = await leakedAfterEnd(0, "Paris");
		const stray = edgegap.foreign([UNKNOWN_TAG], V);
		expect(await sweep(env)).toMatchObject({ listed: 2, stopped: 2, failed: 0, error: null });
		expect(running(ended)).toBe(false);
		expect(running(stray.id)).toBe(false);
	});

	it("keeps a live server whose history writes were all lost: ownership is the registry's", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0);
		await env.DB.prepare("DELETE FROM sessions").run();
		const later = Date.now() + 10 * MINUTE;
		expect(await sweep(env, later)).toMatchObject({ listed: 1, stopped: 0, error: null });
		expect(await sweep(env, later + MINUTE)).toMatchObject({ listed: 1, stopped: 0 });
		expect(running(s.deploymentId)).toBe(true);
	});

	it("keeps a live server's deployment that also carries a tag no claim owns", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0);
		(edgegap.deployment(0).body.tags as string[]).push(UNKNOWN_TAG);
		expect(await sweep(env)).toMatchObject({ listed: 1, stopped: 0, error: null });
		expect(running(s.deploymentId)).toBe(true);
	});

	it("gives every stop a turn when more keep failing than one sweep may try", async () => {
		const strays = Array.from({ length: SWEEP_MAX_STOPS + 1 }, (_, i) => edgegap.foreign([`om-${i.toString(16).padStart(32, "0")}`], V));
		const ids = strays.map((d) => d.id).sort();
		const last = ids[ids.length - 1];
		for (const id of ids.slice(0, -1)) edgegap.refuseStops.add(id);
		const period = ids.length * MINUTE;
		const t0 = Math.ceil(Date.now() / period) * period; // this minute's rotation starts at the first id
		expect(await sweep(env, t0)).toMatchObject({ stopped: 0, failed: SWEEP_MAX_STOPS });
		expect(running(last)).toBe(true);
		expect(await sweep(env, t0 + MINUTE)).toMatchObject({ stopped: 1, failed: SWEEP_MAX_STOPS - 1 });
		expect(running(last)).toBe(false);
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

		const later = Date.now() + 10 * MINUTE;
		expect(await sweep(env, later)).toMatchObject({ listed: 3, stopped: 0, error: null });
		expect(await sweep(env, later + MINUTE)).toMatchObject({ listed: 3, stopped: 0 });
		for (const d of [ready, starting, unknown]) expect(running(d.deploymentId)).toBe(true);
		expect(edgegap.count("DELETE", "/v1/stop/")).toBe(0);
	});

	it("retries a failed stop on the next sweep", async () => {
		const ended = await leakedAfterEnd(0, "Paris");
		const stray = edgegap.foreign([UNKNOWN_TAG], V);
		edgegap.refuseStops = new Set([ended, stray.id]);
		expect(await sweep(env)).toMatchObject({ stopped: 0, failed: 2, error: "stop: stop HTTP 503" });
		expect(running(ended)).toBe(true);
		expect(running(stray.id)).toBe(true);
		edgegap.refuseStops.clear();
		expect(await sweep(env)).toMatchObject({ stopped: 2, failed: 0, error: null });
		expect(running(ended)).toBe(false);
		expect(running(stray.id)).toBe(false);
		expect(await sweep(env)).toMatchObject({ listed: 0, stopped: 0 });
	});

	it("follows the listing's pages", async () => {
		edgegap.maxPageSize = 2;
		const strays = Array.from({ length: 7 }, (_, i) => edgegap.foreign([`om-${i.toString(16).padStart(32, "0")}`], V));
		expect(await sweep(env)).toMatchObject({ listed: 7, stopped: 7 });
		expect(edgegap.count("GET", "/v1/deployments?page=")).toBe(4);
		for (const d of strays) expect(running(d.id)).toBe(false);
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
		expect(page).toMatch(/2 tagged deployments, 0 unattributed, 1 stopped, 1 stops failed; last error: stop: stop HTTP 0/);
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
