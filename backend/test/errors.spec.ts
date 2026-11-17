import { runInDurableObject } from "cloudflare:test";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Edgegap, edgegapFor } from "../src/edgegap";
import { config } from "../src/config";
import worker from "../src/index";
import { adminPage } from "../src/admin";
import { stopWithEvidence } from "../src/evidence";
import { lastSweep, sweep } from "../src/sweep";
import { call, env, join, registry } from "./helpers";

const V = "0.5.0";
const cfg = config(env);
const deploy = { version: V, city: "Paris", tag: "fixture", clientIp: null, env: [], webhookUrl: "" };

beforeEach(() => { vi.mocked(fetch).mockImplementation((input, init) => edgegap.fetch(input, init)); });
afterEach(() => {
	for (const method of ["deploy", "locations", "deployments"] as const) {
		if (vi.isMockFunction(Edgegap.prototype[method])) vi.mocked(Edgegap.prototype[method]).mockRestore();
	}
});

describe("outside failures have typed recovery; invariant failures fail the invocation", () => {
	it.each(["null", "{}", "{", '{"request_id":42}'])("types malformed deploy JSON %s before Registry reconciliation", async (raw) => {
		vi.mocked(fetch).mockImplementation(async () => new Response(raw));
		await expect(edgegapFor(env, cfg.providerTimeoutMs).deploy(deploy)).rejects.toMatchObject({ name: "ProviderError", op: "deploy", outcome: "unknown" });
	});

	it.each(["null", "{}", '{"data":[]}', '{"data":null}', '{"data":[null]}', '{"data":[{"request_id":42}]}', '{"data":[],"pagination":{"has_next":"yes"}}'])("refuses malformed inventory %s instead of proving absence", async (raw) => {
		vi.mocked(fetch).mockImplementation(async () => new Response(raw));
		const provider = edgegapFor(env, cfg.providerTimeoutMs);
		await expect(provider.findByTag("fixture")).rejects.toMatchObject({ name: "ProviderError", op: "list", outcome: "unknown" });
		await expect(provider.deployments(1)).rejects.toMatchObject({ name: "ProviderError", op: "list", outcome: "unknown" });
	});

	it("network failures are typed at fetch while unexpected exchange exceptions propagate", async () => {
		const provider = edgegapFor(env, cfg.providerTimeoutMs);
		vi.mocked(fetch).mockRejectedValue(new TypeError("fixture network failure"));
		await expect(provider.deploy(deploy)).rejects.toMatchObject({ name: "ProviderError", outcome: "unknown" });
		const invariant = new Error("fixture exchange invariant");
		vi.mocked(fetch).mockRejectedValue(invariant);
		await expect(provider.deploy(deploy)).rejects.toBe(invariant);
	});

	it.each(["deploy", "locations"] as const)("an internal %s exception fails the player's invocation", async (method) => {
		vi.spyOn(Edgegap.prototype, method).mockRejectedValue(new TypeError("fixture internal invariant"));
		await expect(join({ version: V, locations: ["Paris"], pings: {} })).rejects.toThrow("fixture internal invariant");
	});

	it("an internal refresh exception cannot serve the stale catalog", async () => {
		await registry(V).locations(V);
		await runInDurableObject(registry(V), (_, state) => { state.storage.sql.exec("UPDATE catalog SET fetched_at = 0"); });
		vi.spyOn(Edgegap.prototype, "locations").mockRejectedValue(new Error("fixture catalog invariant"));
		await runInDurableObject(registry(V), async (reg) => { await expect(reg.locations(V)).rejects.toThrow("fixture catalog invariant"); });
	});

	it("cached JSON from an earlier build is a typed refusal at its read edge", async () => {
		await runInDurableObject(registry(V), (_, state) => {
			state.storage.sql.exec("INSERT INTO catalog VALUES ('edgegap', ?, ?)", '{"locations":null}', Date.now());
		});
		expect(await registry(V).locations(V)).toMatchObject({ ok: false, error: expect.stringContaining("catalogCache") });
	});

	it("scheduled invariant failures reject the platform invocation", async () => {
		vi.spyOn(Edgegap.prototype, "deployments").mockRejectedValue(new Error("fixture sweep invariant"));
		await expect(worker.scheduled({} as ScheduledController, env)).rejects.toThrow("fixture sweep invariant");
	});

	it("admin shows transport unavailability but propagates a snapshot invariant", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		let failure: Error = Object.assign(new Error("fixture transport"), { retryable: true });
		const disconnected = { ...env, REGISTRY: { getByName: () => ({ snapshot: async () => { throw failure; } }) } as unknown as typeof env.REGISTRY };
		expect(await adminPage(disconnected)).toContain("snapshot unavailable");
		failure = new Error("fixture snapshot invariant");
		await expect(adminPage(disconnected)).rejects.toThrow("fixture snapshot invariant");
	});

	it("storage bugs cannot become failed evidence, an absent sweep, or a successful cron", async () => {
		const invariant = new Error("fixture SQL invariant");
		const broken = { ...env, DB: { prepare: () => { throw invariant; }, batch: () => { throw invariant; } } as unknown as D1Database };
		const provider = edgegapFor(env, cfg.providerTimeoutMs);
		const host = edgegap.foreign([], V);
		await expect(stopWithEvidence(broken, provider, "fixture", host.id, cfg.providerTimeoutMs)).rejects.toBe(invariant);
		expect(host.status).toBe("Status.READY");
		await expect(lastSweep(broken)).rejects.toBe(invariant);
		await expect(sweep(broken)).rejects.toBe(invariant);
	});

	it("malformed client JSON is visibly refused", async () => {
		const response = await worker.fetch(new Request("https://api.overmatch.test/v1/join", { method: "POST", body: "{" }), env);
		expect(response.status).toBe(400);
		expect(await response.json()).toEqual({ error: "body must be a JSON object" });
		expect((await call("POST", "/v1/join", null)).status).toBe(400);
	});
});
