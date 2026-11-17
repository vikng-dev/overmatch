import { runInDurableObject } from "cloudflare:test";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Edgegap, edgegapFor } from "../src/edgegap";
import { config, SERVER_SETTING_DOMAINS } from "../src/config";
import worker from "../src/index";
import { adminPage } from "../src/admin";
import { stopWithEvidence } from "../src/evidence";
import { recordEvent } from "../src/history";
import { lastSweep, sweep } from "../src/sweep";
import { call, env, join, launched, registry } from "./helpers";

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
	it.each(["null", "{}", "{", '{"request_id":42}', JSON.stringify({ request_id: "\ud800" })])("types malformed deploy JSON %s before Registry reconciliation", async (raw) => {
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
	it("an interrupted incoming body is a logged request refusal before matchmaking", async () => {
		const log = vi.spyOn(console, "warn");
		const stream = new ReadableStream({ start(controller) { controller.error(new Error("fixture disconnected body")); } });
		const res = await worker.fetch(new Request("https://api.overmatch.test/v1/join", { method: "POST", body: stream }), env);
		expect(res.status).toBe(400);
		expect(await res.json()).toEqual({ error: "request body could not be read" });
		expect(log).toHaveBeenCalledWith("request refused", "bodyRead");
		expect(edgegap.count("POST", "/v1/deploy")).toBe(0);
	});

	it.each(["/v1/servers/%/heartbeat", "/v1/servers/%/end", `/v1/edgegap/webhook/%/${"a".repeat(32)}`])("malformed path encoding is refused at %s", async (path) => {
		const res = await worker.fetch(new Request(`https://api.overmatch.test${path}`, { method: "POST", body: "{}" }), env);
		expect(res.status).toBe(400);
		expect(await res.json()).toEqual({ error: "invalid path encoding" });
	});

	it.each([
		["EDGEGAP_API", "not-a-url"], ["EDGEGAP_TOKEN", "fixture\r\nheader"],
		["EDGEGAP_APP", ""], ["EDGEGAP_PORT_NAME", ""],
		["CAPACITY", "NaN"], ["CAPACITY", "1.5"], ["EMPTY_EXIT_S", "0"], ["HEARTBEAT_DEADLINE_S", "1e308"],
		["ACCESS_TEAM_DOMAIN", "///"], ["ACCESS_AUD", ""],
		["ACCESS_TEAM_DOMAIN", "not-a-url"], ["PROVIDER", "unknown"], ["LOCATION_HINT", "unknown"],
	])("operator %s is refused at request startup without logging its value", async (key, value) => {
		const log = vi.spyOn(console, "error");
		const res = await worker.fetch(new Request("https://api.overmatch.test/v1/locations?version=fixture"), { ...env, [key]: value });
		expect(res.status).toBe(503);
		expect(await res.json()).toEqual({ error: expect.stringContaining(`config: ${key}:`) });
		expect(log).toHaveBeenCalledWith("configuration refused", expect.stringContaining(`config: ${key}:`));
		// Numeric bounds belong in the diagnostic; opaque malformed text must never be echoed.
		if (value && !Number.isFinite(Number(value))) expect(log.mock.calls.flat().join(" ")).not.toContain(value);
	});

	it.each([
		["players", -1], ["players", 0.5], ["players", 1e100], ["players", Number.MAX_SAFE_INTEGER],
		["players", SERVER_SETTING_DOMAINS.CAPACITY.max + 1],
		["capacity", 0], ["capacity", 1.5], ["capacity", 1e100], ["capacity", SERVER_SETTING_DOMAINS.CAPACITY.max + 1],
	])("heartbeat %s=%s is refused before Registry SQL can see it", async (field, value) => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const server = launched();
		const before = await registry(V).snapshot();
		const log = vi.spyOn(console, "warn");
		const res = await call("POST", `/v1/servers/${server.id}/heartbeat`, { players: 0, capacity: SERVER_SETTING_DOMAINS.CAPACITY.max, state: "open", [field]: value }, { authorization: `Bearer ${server.secret}` });
		expect(res.status).toBe(400);
		expect(await res.json()).toEqual({ error: expect.stringContaining(`${field}:`) });
		expect(log).toHaveBeenCalledWith("request refused", "fields");
		expect(await registry(V).snapshot()).toEqual(before);
	});

	it("heartbeat counts admit both ends of the server-owned integer domain", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const server = launched();
		for (const capacity of [SERVER_SETTING_DOMAINS.CAPACITY.min, SERVER_SETTING_DOMAINS.CAPACITY.max]) {
			for (const players of [0, capacity]) {
				const res = await call("POST", `/v1/servers/${server.id}/heartbeat`, { players, capacity, state: "open" }, { authorization: `Bearer ${server.secret}` });
				expect(res.status).toBe(204);
				expect((await registry(V).snapshot())[0]).toMatchObject({ players, capacity });
			}
		}
	});

	it("a heartbeat cannot report more players than its own capacity", async () => {
		const id = "Zml4dHVyZQ." + "a".repeat(32);
		const res = await call("POST", `/v1/servers/${id}/heartbeat`, { players: 2, capacity: 1, state: "open" });
		expect(res.status).toBe(400);
		expect(await res.json()).toEqual({ error: "players: expected an integer from 0 to capacity" });
	});

	it("ping entries are bounded by the location-selection budget", async () => {
		const pings = Object.fromEntries(Array.from({ length: 65 }, (_, i) => [`fixture-${i}`, 1]));
		expect((await call("POST", "/v1/join", { version: V, locations: ["Paris"], pings })).status).toBe(400);
		expect(edgegap.calls).toHaveLength(0);
	});

	it.each([-1, 0.5, 2 ** 32, 1e100])("ping %s is refused before placement", async (value) => {
		const res = await call("POST", "/v1/join", { version: V, locations: ["Paris"], pings: { Paris: value } });
		expect(res.status).toBe(400);
		expect(await res.json()).toEqual({ error: expect.stringContaining("integer ms") });
		expect(edgegap.calls).toHaveLength(0);
	});

	it.each([0, 2 ** 32 - 1])("a u32 ping boundary %s is admitted for placement", async (value) => {
		await join({ version: V, locations: ["Paris", "London"], pings: { Paris: value } });
		expect(edgegap.deployment(0).body.filters).toEqual([{ field: "city", values: ["Paris"], filter_type: "any" }]);
	});

	it.each([["empty", ""], ["too long", "x".repeat(129)], ["unpaired surrogate", "\ud800"]])("location and ping ids share the same Unicode string domain: %s", async (_label, id) => {
		for (const payload of [{ version: V, locations: [id] }, { version: V, locations: ["Paris"], pings: { [id]: 1 } }]) {
			expect((await call("POST", "/v1/join", payload)).status).toBe(400);
		}
		expect(edgegap.calls).toHaveLength(0);
	});

	it.each(["reason", "crash"])("malformed Unicode %s is refused before Registry work", async (field) => {
		const id = "Zml4dHVyZQ." + "a".repeat(32);
		expect((await call("POST", `/v1/servers/${id}/end`, { [field]: "\ud800" })).status).toBe(400);
	});

	it("admin D1 outages are typed while SQL bugs still fail the invocation", async () => {
		let failure = new Error("D1_ERROR: Network connection lost.");
		const unavailable = { ...env, DB: { prepare: () => { throw failure; } } as unknown as D1Database };
		await expect(adminPage(unavailable)).rejects.toMatchObject({ name: "ProviderError", op: "adminVersions:storage" });
		failure = new Error("D1_ERROR: no such table: sessions");
		await expect(adminPage(unavailable)).rejects.toBe(failure);
	});

	it.each(["locations", "join", "ticket", "heartbeat", "end", "webhook"])("HTTP %s refuses platform RPC outages and propagates remote bugs", async (method) => {
		const id = "Zml4dHVyZQ." + "a".repeat(32);
		const [path, payload] = method === "locations" ? ["/v1/locations?version=fixture", undefined]
			: method === "join" ? ["/v1/join", { version: "fixture", locations: ["Paris"] }]
			: method === "ticket" ? ["/v1/join/status", { ticket: id }]
			: method === "webhook" ? [`/v1/edgegap/webhook/${id}/${"a".repeat(32)}`, {}]
			: [`/v1/servers/${id}/${method}`, { players: 0, capacity: 1, state: "open" }];
		let failure: Error = Object.assign(new Error("fixture unavailable"), { retryable: true });
		const unavailable = { ...env, REGISTRY: { getByName: () => ({ [method]: async () => { throw failure; } }) } as unknown as typeof env.REGISTRY };
		const req = () => new Request(`https://api.overmatch.test${path}`, { method: payload === undefined ? "GET" : "POST", body: payload === undefined ? undefined : JSON.stringify(payload) });
		expect((await worker.fetch(req(), unavailable)).status).toBe(503);
		failure = new Error("fixture RPC invariant");
		await expect(worker.fetch(req(), unavailable)).rejects.toBe(failure);
	});

	it("history logs infrastructure downtime but propagates SQL bugs to its caller", async () => {
		let failure = new Error("D1_ERROR: Replica disconnected from primary.");
		const db = { prepare: () => { throw failure; } } as unknown as D1Database;
		const event = { kind: "ready", serverId: "fixture", at: 0 } as const;
		const log = vi.spyOn(console, "error");
		await recordEvent(db, event, cfg.providerTimeoutMs);
		expect(log).toHaveBeenCalledWith("recordEvent failed", "ready", "fixture", expect.stringContaining("history:storage"));
		failure = new Error("D1_ERROR: no such table: sessions");
		await expect(recordEvent(db, event, cfg.providerTimeoutMs)).rejects.toBe(failure);
	});

	it("consumed bodies remain an internal invariant failure", async () => {
		const req = new Request("https://api.overmatch.test/v1/join", { method: "POST", body: "{}" });
		await req.text();
		await expect(worker.fetch(req, env)).rejects.toThrow("request body must be read once");
	});

	it.each([
		["/v1/join", { version: "fixture", locations: ["Paris"], pings: null }],
		["/v1/join", { version: "fixture\n", locations: ["Paris"] }],
		["/v1/servers/Zml4dHVyZQ.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/end", { reason: 42 }],
		["/v1/servers/Zml4dHVyZQ.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/end", []],
		["/v1/edgegap/webhook/Zml4dHVyZQ.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", { request_id: 42 }],
	])("malformed optional fields are refused instead of treated as absent: %s", async (path, payload) => {
		const res = await worker.fetch(new Request(`https://api.overmatch.test${path}`, { method: "POST", body: JSON.stringify(payload) }), env);
		expect(res.status).toBe(400);
	});

	it.each(["not-an-ip", "999.0.0.1", "1::2::3"])("malformed forwarded client address %s is refused before provider work", async (ip) => {
		const res = await call("POST", "/v1/join", { version: V, locations: ["Paris"] }, { "cf-connecting-ip": ip });
		expect(res.status).toBe(400);
		expect(await res.json()).toEqual({ error: "invalid cf-connecting-ip header" });
		expect(edgegap.calls).toHaveLength(0);
	});

	it.each(["192.0.2.1", "2001:db8::1", "::ffff:192.0.2.1"])("a checked forwarded address %s reaches placement unchanged", async (ip) => {
		expect((await call("POST", "/v1/join", { version: V, locations: ["Paris"] }, { "cf-connecting-ip": ip })).status).toBe(200);
		expect(edgegap.deployment(0).body.ip_list).toEqual([ip]);
	});
});
