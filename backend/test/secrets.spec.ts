import { describe, expect, it } from "vitest";
import { adminPage } from "../src/admin";
import { bringUp, call, env, join, launched, registry, ticket } from "./helpers";

const V = "0.5.0";

describe("secrets reach only their own callers", () => {
	it("a ticket returns its own server's key, never another's, and none while starting", async () => {
		const ta = await join({ version: V, locations: ["Paris"], pings: {} });
		const tb = await join({ version: V, locations: ["Singapore"], pings: {} });
		expect(await ticket(ta)).toEqual({ state: "starting" });

		const a = await bringUp(0);
		const b = await bringUp(1);
		expect(a.key).not.toBe(b.key);
		expect((await ticket(ta)).key).toBe(a.key);
		expect((await ticket(tb)).key).toBe(b.key);
	});

	it("no other response carries a key or a report secret", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0, 2);

		const bodies = [
			await (await call("GET", `/v1/locations?version=${V}`)).text(),
			JSON.stringify(await registry(V).snapshot()),
			await adminPage(env),
		];
		for (const body of bodies) {
			expect(body).not.toContain(s.key);
			expect(body).not.toContain(s.secret);
		}
		const snap = (await registry(V).snapshot())[0] as unknown as Record<string, unknown>;
		expect(Object.keys(snap)).not.toContain("netcode_key");
		expect(Object.keys(snap)).not.toContain("secret_hash");
	});

	it("each server gets its own key and report secret", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		await join({ version: V, locations: ["London"], pings: {} });
		const a = launched(0);
		const b = launched(1);
		expect(a.key).not.toBe(b.key);
		expect(a.secret).not.toBe(b.secret);
		expect(a.secret).toMatch(/^[0-9a-f]{64}$/);
	});
});
