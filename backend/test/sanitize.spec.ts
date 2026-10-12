import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { adminPage } from "../src/admin";
import { env, join, launched, ticket, webhook } from "./helpers";

const V = "0.5.0";
let logged: string[] = [];
let spies: { mockRestore(): void }[] = [];

beforeEach(() => {
	logged = [];
	spies = [];
	for (const level of ["log", "info", "warn", "error", "debug"] as const) {
		spies.push(vi.spyOn(console, level).mockImplementation((...args: unknown[]) => {
			logged.push(args.map((a) => (a instanceof Error ? `${a.name}: ${a.message}` : typeof a === "string" ? a : JSON.stringify(a))).join(" "));
		}));
	}
});

afterEach(() => {
	for (const spy of spies) spy.mockRestore();
});

/** Every secret a deploy body carries. */
function secretsOf(n: number): string[] {
	const d = edgegap.deployment(n);
	return [edgegap.env(d, "OVERMATCH_NETCODE_KEY"), edgegap.env(d, "OVERMATCH_REPORT_SECRET"), "test-webhook-secret"];
}

describe("provider error text never reaches clients, history or logs", () => {
	it("a refused deploy whose error echoes the request", async () => {
		edgegap.echoInErrors = true;
		edgegap.refuse.add("Paris");
		const t = await join({ version: V, locations: ["Paris"], pings: {} });
		const out = await ticket(t);
		expect(out).toEqual({ state: "failed", reason: "no capacity in the selected location" });
		const page = await adminPage(env);
		const everything = [JSON.stringify(out), page, ...logged].join("\n");
		expect(everything).not.toMatch(/OVERMATCH_|request:|webhook/);
	});

	it("an error status whose detail echoes the deployment", async () => {
		edgegap.echoInErrors = true;
		const t = await join({ version: V, locations: ["Paris"], pings: {} });
		const secrets = secretsOf(0);
		edgegap.deployment(0).status = "Status.ERROR";
		await webhook(launched().id);
		const out = await ticket(t);
		expect(out).toEqual({ state: "failed", reason: "the server failed to start" });
		const everything = [JSON.stringify(out), await adminPage(env), ...logged].join("\n");
		for (const s of secrets) expect(everything).not.toContain(s);
		expect(everything).not.toMatch(/Not enough space/);
	});
});
