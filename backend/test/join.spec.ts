import { describe, expect, it } from "vitest";
import { bringUp, call, join, launched, ticket } from "./helpers";

const V = "0.5.0";

describe("join-or-create", () => {
	it("concurrent joins start exactly one server and all attach to it", async () => {
		const release = edgegap.holdDeploys();
		const joins = Array.from({ length: 5 }, () => join({ version: V, locations: ["Singapore"], pings: { Singapore: 40 } }));
		// Let every join reach the registry while the first deploy call is still in flight.
		await new Promise((r) => setTimeout(r, 50));
		release();
		const tickets = await Promise.all(joins);

		expect(edgegap.count("POST", "/v1/deploy")).toBe(1);
		for (const t of tickets) expect(await ticket(t)).toEqual({ state: "starting" });

		const s = await bringUp(0);
		for (const t of tickets) {
			expect(await ticket(t)).toEqual({ state: "ready", address: "198.51.100.1", port: 31001, key: s.key });
		}
	});

	it("a starting server absorbs later joins", async () => {
		const a = await join({ version: V, locations: ["Singapore"], pings: { Singapore: 40 } });
		const b = await join({ version: V, locations: ["Singapore", "Tokyo"], pings: { Singapore: 40, Tokyo: 110 } });
		expect(edgegap.count("POST", "/v1/deploy")).toBe(1);
		const s = await bringUp(0);
		expect((await ticket(a)).key).toBe(s.key);
		expect((await ticket(b)).key).toBe(s.key);
	});

	it("starts in the lowest-ping selected location, with the city filter and OVERMATCH_* env", async () => {
		await join({ version: V, locations: ["Mumbai", "Singapore", "Paris"], pings: { Mumbai: 144, Singapore: 45, Paris: 228 } });
		const d = edgegap.deployment(0);
		expect(d.body.app_name).toBe("Overmatch");
		expect(d.body.version_name).toBe(V);
		expect(d.body.filters).toEqual([{ field: "city", values: ["Singapore"], filter_type: "any" }]);
		const vars = d.body.env_vars as { key: string; value: string; is_hidden: boolean }[];
		expect(vars.map((v) => v.key).sort()).toEqual([
			"OVERMATCH_API",
			"OVERMATCH_CAPACITY",
			"OVERMATCH_EMPTY_EXIT_S",
			"OVERMATCH_HEARTBEAT_INTERVAL_S",
			"OVERMATCH_NETCODE_KEY",
			"OVERMATCH_REPORT_SECRET",
			"OVERMATCH_SERVER_ID",
		]);
		expect(vars.filter((v) => v.is_hidden).map((v) => v.key).sort()).toEqual(["OVERMATCH_NETCODE_KEY", "OVERMATCH_REPORT_SECRET"]);
		expect(launched().key).toMatch(/^[0-9a-f]{64}$/);
		expect(d.body.webhook_url).toMatch(/^https:\/\/api\.overmatch\.test\/v1\/edgegap\/webhook\?server=/);
	});

	it("a location without a ping ranks after every location with one", async () => {
		await join({ version: V, locations: ["Mumbai", "Paris"], pings: { Paris: 228 } });
		expect(edgegap.deployment(0).body.filters).toEqual([{ field: "city", values: ["Paris"], filter_type: "any" }]);
	});

	it("prefers the lowest-ping open server, then the fullest", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		await join({ version: V, locations: ["Singapore"], pings: {} });
		await bringUp(0, 3); // Paris, 3 players
		const sg = await bringUp(1, 1); // Singapore, 1 player
		const t = await join({ version: V, locations: ["Paris", "Singapore"], pings: { Paris: 200, Singapore: 40 } });
		expect((await ticket(t)).key).toBe(sg.key);
		expect(edgegap.count("POST", "/v1/deploy")).toBe(2);
	});

	it("among equal pings the fuller non-full server wins", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		await join({ version: V, locations: ["London"], pings: {} });
		await bringUp(0, 2);
		const london = await bringUp(1, 6);
		const t = await join({ version: V, locations: ["Paris", "London"], pings: { Paris: 30, London: 30 } });
		expect((await ticket(t)).key).toBe(london.key);
	});

	it("full servers are skipped; a reservation counts before the heartbeat shows the player", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const full = await bringUp(0, 10);
		const t = await join({ version: V, locations: ["Paris"], pings: {} });
		expect(edgegap.count("POST", "/v1/deploy")).toBe(2);
		const second = await bringUp(1, 9);
		expect((await ticket(t)).key).toBe(second.key);
		expect((await ticket(t)).key).not.toBe(full.key);
		// `t` is reserved on `second` (9 + 1 = 10): the next join must start a third server.
		await join({ version: V, locations: ["Paris"], pings: {} });
		expect(edgegap.count("POST", "/v1/deploy")).toBe(3);
	});

	it("a closing server is not joinable", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0, 1);
		await call("POST", `/v1/servers/${s.id}/heartbeat`, { players: 1, capacity: 10, state: "closing" }, { authorization: `Bearer ${s.secret}` });
		await join({ version: V, locations: ["Paris"], pings: {} });
		expect(edgegap.count("POST", "/v1/deploy")).toBe(2);
	});

	it("versions never share servers", async () => {
		await join({ version: "0.5.0", locations: ["Paris"], pings: {} });
		await join({ version: "0.5.1", locations: ["Paris"], pings: {} });
		expect(edgegap.count("POST", "/v1/deploy")).toBe(2);
		expect(edgegap.deployment(1).body.version_name).toBe("0.5.1");
	});

	it("a refused deploy falls back to the next selected location, once", async () => {
		edgegap.refuse.add("Frankfurt");
		const t = await join({ version: V, locations: ["Frankfurt", "Paris"], pings: { Frankfurt: 10, Paris: 20 } });
		expect(edgegap.deployments.size).toBe(1);
		expect(edgegap.deployment(0).body.filters).toEqual([{ field: "city", values: ["Paris"], filter_type: "any" }]);
		expect(await ticket(t)).toEqual({ state: "starting" });
	});

	it("a ticket fails when every attempt is refused", async () => {
		edgegap.refuse.add("Frankfurt");
		edgegap.refuse.add("Paris");
		const t = await join({ version: V, locations: ["Frankfurt", "Paris"], pings: { Frankfurt: 10, Paris: 20 } });
		const out = await ticket(t);
		expect(out.state).toBe("failed");
		expect(out.reason).toMatch(/Not enough space/);
	});

	it("rejects unknown locations and malformed bodies", async () => {
		const bad = await call("POST", "/v1/join", { version: V, locations: ["Atlantis"], pings: {} });
		expect(bad.status).toBe(400);
		expect(await bad.json()).toEqual({ error: "unknown locations: Atlantis" });
		const noVersion = await call("POST", "/v1/join", { locations: ["Paris"], pings: {} });
		expect(noVersion.status).toBe(400);
		await noVersion.text();
		const badPing = await call("POST", "/v1/join", { version: V, locations: ["Paris"], pings: { Paris: -1 } });
		expect(badPing.status).toBe(400);
		await badPing.text();
		expect(edgegap.count("POST", "/v1/deploy")).toBe(0);
	});

	it("an unknown ticket is 404", async () => {
		const res = await call("GET", "/v1/join/MC41LjA.00000000000000000000000000000000");
		expect(res.status).toBe(404);
		await res.text();
	});
});
