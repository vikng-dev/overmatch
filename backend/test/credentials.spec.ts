// No credential travels in a URL: request URLs are what invocation logs record.

import { describe, expect, it } from "vitest";
import wrangler from "../wrangler.jsonc?raw";
import { bringUp, call, join, launched } from "./helpers";

const V = "0.5.0";

describe("credentials stay out of URLs", () => {
	it("a ticket is redeemed from the request body, and the ticket-in-path route is gone", async () => {
		const t = await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp(0);
		const res = await call("POST", "/v1/join/status", { ticket: t });
		expect(res.status).toBe(200);
		expect(await res.json()).toMatchObject({ state: "ready", key: s.key });
		const old = await call("GET", `/v1/join/${t}`);
		expect(old.status).toBe(404);
		await old.text();
	});

	it("the webhook URL carries no shared secret, only a per-server path that triggers a status read", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		await join({ version: V, locations: ["London"], pings: {} });
		const urls = [0, 1].map((n) => new URL(edgegap.deployment(n).body.webhook_url as string));
		for (const u of urls) {
			expect(u.search).toBe("");
			expect(u.pathname).toMatch(/^\/v1\/edgegap\/webhook\/[^/]+\/[0-9a-f]{32}$/);
		}
		expect(urls[0].pathname.split("/").pop()).not.toBe(urls[1].pathname.split("/").pop());

		const statusCalls = edgegap.count("GET", "/v1/status/");
		const forged = urls[0].pathname.replace(/[0-9a-f]{32}$/, "0".repeat(32));
		const res = await call("POST", forged, { request_id: launched(0).deploymentId });
		expect(res.status).toBe(404);
		await res.text();
		expect(edgegap.count("GET", "/v1/status/")).toBe(statusCalls);

		const ok = await call("POST", urls[0].pathname, {});
		expect(ok.status).toBe(204);
		expect(edgegap.count("GET", "/v1/status/")).toBe(statusCalls + 1);
	});

	it("invocation logs, which record request URLs, are off", () => {
		expect(wrangler).toMatch(/"invocation_logs"\s*:\s*false/);
	});
});
