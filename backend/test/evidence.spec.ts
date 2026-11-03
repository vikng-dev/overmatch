import { runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { describe, expect, it, vi } from "vitest";
import { config } from "../src/config";
import { edgegapFor } from "../src/edgegap";
import { SERVER_LOG_MAX_BYTES, stopWithEvidence } from "../src/evidence";
import { idOf } from "../src/ids";
import { sweep } from "../src/sweep";
import { age, bringUp, call, env, join, launched, registry } from "./helpers";

const V = "0.5.0";

type Archive = { server_id: string; deployment_id: string; captured_at: number; log: string | null; truncated: number; error: string | null };

async function archive(deploymentId: string): Promise<Archive | null> {
	return env.DB.prepare("SELECT * FROM server_logs WHERE deployment_id = ?1").bind(deploymentId).first<Archive>();
}

async function end(s: { id: string; secret: string }): Promise<void> {
	expect((await call("POST", `/v1/servers/${s.id}/end`, { reason: "empty" }, { authorization: `Bearer ${s.secret}` })).status).toBe(204);
}

describe("stopped match servers leave evidence for the operator", () => {
	it("preserves diagnostic text so the operator can read paths, timestamps and stats", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		const lines = [
			"overmatch::net::server: server: client connected",
			"src/buildings/fracture.rs:2100:9",
			"12:45:41.925415Z",
			"lightyear_netcode::server_plugin",
			'server_stats {"tick_us":{"p50":123.4},"debris":{"live":2,"sleeping":18}}',
			"SimTick(3963)",
			"0.7.0-alpha.10",
			"panic at fracture::weigh",
			"::apply",
			"::",
		];
		edgegap.logBody = { logs: lines.join("\n") };
		await end(s);
		expect((await archive(s.deploymentId))!.log).toBe(lines.join("\n"));
	});

	it("masks whole IPv4 and IPv6 address tokens before the operator's archive is stored", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		edgegap.logBody = { logs: ["42.113.67.73:57658", "[::ffff:1.2.3.4]:5888", "2001:db8::1", "fe80::1%en0", "::1"].join("\n") };
		await end(s);
		expect((await archive(s.deploymentId))!.log).toBe(["[ip]:57658", "[[ip]]:5888", "[ip]", "[ip]", "[ip]"].join("\n"));
	});

	it("archives stdout and crash fields before stop, with addresses and credentials masked in D1", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		edgegap.logBody = {
			logs: `Received UDP packet from new address 203.0.113.40:5888\npeer [2001:db8::1]:5888\npeer ::ffff:192.0.2.1\n${s.key}\nAuthorization: Bearer ${s.secret}\n${env.EDGEGAP_TOKEN}\nhttps://example.test/credential\nserver_stats {"ticks":320}`,
			crash_logs: "panic at 198.51.100.4:33\nbacktrace fixture",
			crash_data: { exit_code: 1, message: "peer 192.0.2.20" },
		};
		await end(s);
		const row = (await archive(s.deploymentId))!;
		expect(row).toMatchObject({ server_id: s.id, error: null, truncated: 0 });
		for (const sensitive of ["203.0.113.40", "2001:db8::1", "192.0.2.1", "198.51.100.4", "192.0.2.20", s.key, s.secret, env.EDGEGAP_TOKEN, "example.test"]) {
			expect(JSON.stringify(row)).not.toContain(sensitive);
		}
		expect(row.log).toContain("new address [ip]:5888");
		expect(row.log).toContain('server_stats {"ticks":320}');
		expect(row.log).toContain("--- crash_logs ---\npanic at [ip]:33\nbacktrace fixture");
		expect(row.log).toContain('--- crash_data ---\n{"exit_code":1,"message":"peer [ip]"}');
		const capture = edgegap.calls.findIndex((c) => c.path.includes("/container-logs"));
		const stop = edgegap.calls.findIndex((c) => c.method === "DELETE");
		expect(capture).toBeLessThan(stop);
	});

	it("bounds the stored UTF-8 tail in bytes and records truncation without losing its end", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		edgegap.logBody = { logs: "old head\n" + "界".repeat(SERVER_LOG_MAX_BYTES) + "\nlast reading" };
		await end(s);
		const row = (await archive(s.deploymentId))!;
		expect(row.truncated).toBe(1);
		expect(new TextEncoder().encode(row.log!).length).toBeLessThanOrEqual(SERVER_LOG_MAX_BYTES);
		expect(new TextEncoder().encode(row.log!).length).toBeGreaterThan(SERVER_LOG_MAX_BYTES - new TextEncoder().encode("界").length);
		expect(row.log).not.toContain("old head");
		expect(row.log).not.toContain("\ufffd");
		expect(row.log!.endsWith("last reading")).toBe(true);
	});

	for (const failure of ["404", "503", "malformed", "timeout"] as const) {
		it(`records a ${failure} log failure and still stops the server`, async () => {
			await join({ version: V, locations: ["Paris"], pings: {} });
			const s = await bringUp();
			if (failure === "timeout") edgegap.hangLogs.add(s.deploymentId);
			else if (failure === "malformed") edgegap.logBody = { logs: 123 };
			else edgegap.logStatus = Number(failure);
			const warn = vi.spyOn(console, "warn");
			await end(s);
			expect(edgegap.deployment().status).toBe("Status.TERMINATED");
			expect(await archive(s.deploymentId)).toMatchObject({ log: null, error: `logs HTTP ${failure === "timeout" ? 0 : failure === "malformed" ? 502 : failure}` });
			expect(warn).toHaveBeenCalledWith("server log capture failed; stopping anyway", s.id, s.deploymentId, expect.any(String));
			warn.mockRestore();
		});
	}

	it("does not fetch again on a retried stop once even an empty log is stored", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		edgegap.logBody = { logs: "" };
		edgegap.failStops = 1;
		await end(s);
		const stored = await archive(s.deploymentId);
		await age(V, config(env).statusPollMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(edgegap.count("GET", `/v1/deployment/${s.deploymentId}/container-logs`)).toBe(1);
		expect(await archive(s.deploymentId)).toEqual(stored);
		expect(edgegap.deployment().status).toBe("Status.TERMINATED");
	});

	it("retries failed capture when stop also failed, preserving the later successful log", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		edgegap.logStatus = 503;
		edgegap.failStops = 1;
		await end(s);
		edgegap.logStatus = 200;
		await age(V, config(env).statusPollMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(edgegap.count("GET", `/v1/deployment/${s.deploymentId}/container-logs`)).toBe(2);
		expect(await archive(s.deploymentId)).toMatchObject({ error: null, log: expect.stringContaining("server_stats") });
	});

	it("captures a heartbeat-lost server before the alarm stops it", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		await age(V, config(env).heartbeatDeadlineMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(await archive(s.deploymentId)).toMatchObject({ server_id: s.id, error: null });
		expect(edgegap.deployment().status).toBe("Status.TERMINATED");
	});

	it("records unavailable evidence when the provider reports a process already terminated", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = launched();
		edgegap.deployment().status = "Status.TERMINATED";
		await age(V, config(env).statusPollMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(await archive(s.deploymentId)).toMatchObject({ error: "logs HTTP 404" });
		expect((await registry(V).snapshot())[0].state).toBe("ended");
	});

	it("captures an orphan without a session row using the version and claim tag", async () => {
		const hex = "0123456789abcdef0123456789abcdef";
		const d = edgegap.foreign([`om-${hex}`], V);
		expect(await sweep(env)).toMatchObject({ stopped: 1 });
		expect(await archive(d.id)).toMatchObject({ server_id: idOf(V, hex), error: null, log: expect.stringContaining("server_stats") });
	});

	it("captures a late deploy even after its registry claim was forgotten", async () => {
		// Keep the live deploy call in flight until its claim is removed, probing adopt's late path.
		const release = edgegap.holdDeploys();
		const joining = join({ version: V, locations: ["Paris"], pings: {} });
		while (!(await registry(V).snapshot()).length) await new Promise((r) => setTimeout(r, 1));
		await runInDurableObject(registry(V), (_, state) => { state.storage.sql.exec("DELETE FROM servers"); });
		release();
		await joining;
		const s = launched();
		expect(await archive(s.deploymentId)).toMatchObject({ server_id: s.id, error: null });
		expect(edgegap.deployment().status).toBe("Status.TERMINATED");
	});

	it("a D1 outage cannot veto the provider stop", async () => {
		const d = edgegap.foreign([], V);
		const broken = { ...env, DB: { prepare: () => { throw new Error("D1 unavailable"); } } as unknown as D1Database };
		await stopWithEvidence(broken, edgegapFor(env, config(env).providerTimeoutMs), "fixture", d.id, config(env).providerTimeoutMs);
		expect(d.status).toBe("Status.TERMINATED");
	});

	it("a D1 read or failure-write stall cannot veto the provider stop", async () => {
		const d = edgegap.foreign([], V);
		const stalled = () => new Promise<never>(() => {});
		const broken = { ...env, DB: { prepare: () => ({ bind: () => ({ first: stalled, run: stalled }) }) } as unknown as D1Database };
		await stopWithEvidence(broken, edgegapFor(env, config(env).providerTimeoutMs), "fixture", d.id, config(env).providerTimeoutMs);
		expect(d.status).toBe("Status.TERMINATED");
		expect(edgegap.count("GET", `/v1/deployment/${d.id}/container-logs`)).toBe(0);
	});

	it("cron expires successful and failed archives after the configured window even if listing fails", async () => {
		const now = Date.now();
		const cutoff = now - config(env).serverLogRetentionMs;
		for (const [id, at, log] of [["old", cutoff - 1, "old log"], ["failed", cutoff - 1, null], ["kept", cutoff, "recent log"]] as const) {
			await env.DB.prepare("INSERT INTO server_logs VALUES (?1, ?1, ?2, ?3, 0, NULL)").bind(id, at, log).run();
		}
		edgegap.listingDown = true;
		await sweep(env, now);
		expect(await archive("old")).toBeNull();
		expect(await archive("failed")).toBeNull();
		expect(await archive("kept")).not.toBeNull();
	});
});
