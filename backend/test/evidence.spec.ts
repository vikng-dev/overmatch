import { abortAllDurableObjects, runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { describe, expect, it, vi } from "vitest";
import redactionCases from "./journal-redaction.json";
import { config } from "../src/config";
import { edgegapFor, SERVER_LOG_BODY_MAX_BYTES } from "../src/edgegap";
import { SERVER_LOG_MAX_BYTES, stopWithEvidence, sanitizedTail, recordJournal, recordMissingJournal, evidenceResult } from "../src/evidence";
import { ProviderError } from "../src/provider";
import { idOf } from "../src/ids";
import { sweep } from "../src/sweep";
import { failD1Once, age, bringUp, call, env, join, launched, registry } from "./helpers";

const V = "0.5.0";

type Archive = { server_id: string; deployment_id: string; captured_at: number; log: string | null; truncated: number; error: string | null };

async function archive(deploymentId: string): Promise<Archive | null> {
	return env.DB.prepare("SELECT * FROM server_logs WHERE deployment_id = ?1").bind(deploymentId).first<Archive>();
}

async function end(s: { id: string; secret: string }): Promise<void> {
	expect((await call("POST", `/v1/servers/${s.id}/end`, { reason: "empty" }, { authorization: `Bearer ${s.secret}` })).status).toBe(204);
}

describe("stopped match servers leave evidence for the operator", () => {
	it("the operator cannot push into a provider whose stop owns log pull", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = launched();
		const response = await call("POST", `/v1/servers/${s.id}/journal`, "pushed log", {
			authorization: `Bearer ${s.secret}`, "x-journal-truncated": "0",
		});
		expect(response.status).toBe(410);
		expect(await archive(s.deploymentId)).toBeNull();
		await end(s);
		expect(edgegap.count("GET", `/v1/deployment/${s.deploymentId}/container-logs`)).toBe(1);
	});

	it("records authenticated panic text before log capture, sanitises it, and calls the session a crash", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		const crash = `panic fixture at src/net/hosting.rs:1:2\n${s.secret}\nAuthorization: Bearer fixture\npeer 192.0.2.1\nhttps://example.test/key\n${env.EDGEGAP_TOKEN}`;
		let crashAtLogRead: Promise<{ crash: string } | null> | undefined;
		edgegap.logResponse = () => {
			crashAtLogRead = env.DB.prepare("SELECT crash FROM server_crashes WHERE server_id = ?1").bind(s.id).first<{ crash: string }>();
			return new Response(null, { status: 404 });
		};
		const response = await call("POST", `/v1/servers/${s.id}/end`, { reason: "panic", crash }, { authorization: `Bearer ${s.secret}` });
		expect(response.status).toBe(204);
		const saved = await env.DB.prepare("SELECT * FROM server_crashes WHERE server_id = ?1").bind(s.id).first<{ crash: string; truncated: number }>();
		expect(saved!.crash).toContain("panic fixture at src/net/hosting.rs:1:2");
		for (const secret of [s.secret, "192.0.2.1", "example.test", env.EDGEGAP_TOKEN]) expect(saved!.crash).not.toContain(secret);
		expect(saved!.truncated).toBe(0);
		expect(await crashAtLogRead).toEqual({ crash: saved!.crash });
		expect(await env.DB.prepare("SELECT outcome, reason FROM sessions WHERE server_id = ?1").bind(s.id).first()).toEqual({ outcome: "failed", reason: "the server crashed" });
		expect(await archive(s.deploymentId)).toMatchObject({ log: null, error: "logs HTTP 404" });
	});

	it("refuses unauthenticated and malformed crash input without changing the live server", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		expect((await call("POST", `/v1/servers/${s.id}/end`, { crash: "untrusted" })).status).toBe(401);
		expect((await call("POST", `/v1/servers/${s.id}/end`, { crash: 1 }, { authorization: `Bearer ${s.secret}` })).status).toBe(400);
		expect(await env.DB.prepare("SELECT * FROM server_crashes WHERE server_id = ?1").bind(s.id).first()).toBeNull();
		expect((await registry(V).snapshot()).find((row) => row.id === s.id)).toMatchObject({ state: "ready", outcome: null });
	});

	it("preserves a startup panic while the provider's deployment response is lost", async () => {
		edgegap.loseDeployResponses = 1;
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = launched();
		expect((await registry(V).snapshot()).find((row) => row.id === s.id)!.deployment_id).toBeNull();
		expect((await call("POST", `/v1/servers/${s.id}/end`, { crash: "startup panic fixture" }, { authorization: `Bearer ${s.secret}` })).status).toBe(204);
		expect(await env.DB.prepare("SELECT crash FROM server_crashes WHERE server_id = ?1").bind(s.id).first()).toEqual({ crash: "startup panic fixture" });
	});

	it("keeps the crash cause when a clean stop is in flight or has already closed history", async () => {
		for (const stopFails of [true, false]) {
			await join({ version: V, locations: ["Paris"], pings: {} });
			const s = await bringUp(edgegap.deployments.size - 1);
			if (stopFails) edgegap.refuseStops.add(s.deploymentId);
			await end(s);
			expect((await registry(V).snapshot()).find((row) => row.id === s.id)!.state).toBe(stopFails ? "stopping" : "ended");
			// The released clean-end contract still refuses a duplicate stop.
			expect((await call("POST", `/v1/servers/${s.id}/end`, {}, { authorization: `Bearer ${s.secret}` })).status).toBe(410);
			expect((await call("POST", `/v1/servers/${s.id}/end`, { crash: "late panic fixture" }, { authorization: `Bearer ${s.secret}` })).status).toBe(204);
			expect(await env.DB.prepare("SELECT crash FROM server_crashes WHERE server_id = ?1").bind(s.id).first()).toEqual({ crash: "late panic fixture" });
			expect(await env.DB.prepare("SELECT outcome, reason FROM sessions WHERE server_id = ?1").bind(s.id).first()).toEqual({ outcome: "failed", reason: "the server crashed" });
			edgegap.refuseStops.delete(s.deploymentId);
		}
	});

	it("joins an active log capture so a concurrent crash cannot stop its container early", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		const release = edgegap.holdLogs();
		edgegap.logBody = { logs: "captured before the sole stop" };
		const ending = call("POST", `/v1/servers/${s.id}/end`, { reason: "shutdown" }, { authorization: `Bearer ${s.secret}` });
		await vi.waitFor(() => expect(edgegap.count("GET", `/v1/deployment/${s.deploymentId}/container-logs`)).toBe(1));
		const crashing = call("POST", `/v1/servers/${s.id}/end`, { crash: "concurrent panic fixture" }, { authorization: `Bearer ${s.secret}` });
		let beforeRelease: number;
		try {
			// Fixture window is below the provider's deadline; an unjoined stop completes in it.
			await Promise.race([crashing, new Promise((resolve) => setTimeout(resolve, config(env).providerTimeoutMs / 4))]);
			beforeRelease = edgegap.count("DELETE", `/v1/stop/${s.deploymentId}`);
		} finally {
			release();
		}
		expect((await ending).status).toBe(204);
		expect((await crashing).status).toBe(204);
		expect(beforeRelease).toBe(0);
		expect(edgegap.count("DELETE", `/v1/stop/${s.deploymentId}`)).toBe(1);
		expect((await archive(s.deploymentId))!.log).toBe("captured before the sole stop");
	});

	it("leaves the registry's active capture to its sole stop owner during an orphan sweep", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		const release = edgegap.holdLogs();
		const ending = end(s);
		let sweepStops: number;
		let beforeRelease: number;
		try {
			await vi.waitFor(() => expect(edgegap.count("GET", `/v1/deployment/${s.deploymentId}/container-logs`)).toBe(1));
			sweepStops = (await sweep(env)).stopped;
			beforeRelease = edgegap.count("DELETE", `/v1/stop/${s.deploymentId}`);
		} finally {
			release();
			await ending;
		}
		expect(sweepStops).toBe(0);
		expect(beforeRelease).toBe(0);
		expect(edgegap.count("DELETE", `/v1/stop/${s.deploymentId}`)).toBe(1);
	});

	it("fails a lost crash write visibly while still stopping and replacing waiting join tickets", async () => {
		await join({ version: V, locations: ["Paris", "Frankfurt"], pings: {} });
		const s = launched();
		// Fixture D1 fault affects only the crash sink; stop evidence and session history still work.
		await env.DB.prepare("ALTER TABLE server_crashes RENAME TO fixture_crashes_unavailable").run();
		try {
			await expect(call("POST", `/v1/servers/${s.id}/end`, { crash: "panic fixture" }, { authorization: `Bearer ${s.secret}` })).rejects.toThrow("no such table");
			expect(edgegap.deployment().status).toBe("Status.TERMINATED");
			expect(edgegap.deployments.size).toBe(2);
			expect((await registry(V).snapshot()).find((row) => row.id === s.id)!.state).toBe("ended");
		} finally {
			await env.DB.prepare("ALTER TABLE fixture_crashes_unavailable RENAME TO server_crashes").run();
		}
	});

	it("a D1 outage is a typed end refusal after stop and ticket replacement, across real RPC", async () => {
		await join({ version: V, locations: ["Paris", "Frankfurt"], pings: {} });
		const s = launched();
		await failD1Once(V, "INSERT INTO server_crashes");
		expect((await call("POST", `/v1/servers/${s.id}/end`, { crash: "panic fixture" }, { authorization: `Bearer ${s.secret}` })).status).toBe(503);
		expect(edgegap.deployment().status).toBe("Status.TERMINATED");
		expect(edgegap.deployments.size).toBe(2);
		expect((await registry(V).snapshot()).find((row) => row.id === s.id)!.state).toBe("ended");
	});

	it("bounds a crash's sanitised UTF-8 bytes without corrupting its tail", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		const crash = "old head\n" + "界::1 ".repeat(Math.floor((SERVER_LOG_MAX_BYTES - 128) / 7)) + "\nlast frame";
		expect((await call("POST", `/v1/servers/${s.id}/end`, { crash }, { authorization: `Bearer ${s.secret}` })).status).toBe(204);
		const saved = await env.DB.prepare("SELECT crash, truncated FROM server_crashes WHERE server_id = ?1").bind(s.id).first<{ crash: string; truncated: number }>();
		expect(saved!.truncated).toBe(1);
		expect(new TextEncoder().encode(saved!.crash).length).toBeLessThanOrEqual(SERVER_LOG_MAX_BYTES);
		expect(saved!.crash).not.toContain("old head");
		expect(saved!.crash).not.toContain("\ufffd");
		expect(saved!.crash.endsWith("last frame")).toBe(true);
	});

	it("leaves old-server end reports as clean ends without crash evidence", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		await end(s);
		expect(await env.DB.prepare("SELECT * FROM server_crashes WHERE server_id = ?1").bind(s.id).first()).toBeNull();
		expect(await env.DB.prepare("SELECT outcome, reason FROM sessions WHERE server_id = ?1").bind(s.id).first()).toEqual({ outcome: "ended", reason: "server: empty" });
	});

	it("preserves diagnostic text so the operator can read paths, timestamps and stats", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		const lines = [
			"overmatch::net::server: server: client connected",
			"src/construction/fracture.rs:2100:9",
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

	it("cancels an oversized log stream at the byte limit and still stops the deployment", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		// Fixture chunks expose any read beyond the first over-limit chunk, without allocating a huge body.
		const chunkBytes = 64 * 1024 + 1;
		let bytesRead = 0;
		let cancelled = false;
		edgegap.logResponse = () => new Response(new ReadableStream<Uint8Array>({
			pull(controller) {
				if (bytesRead > SERVER_LOG_BODY_MAX_BYTES) throw new Error("read beyond the oversized chunk");
				const chunk = new Uint8Array(chunkBytes).fill(120);
				if (bytesRead === 0) chunk.set(new TextEncoder().encode('{"logs":"'));
				bytesRead += chunk.byteLength;
				controller.enqueue(chunk);
			},
			cancel() { cancelled = true; },
		}, { highWaterMark: 0 }));
		await end(s);
		expect(cancelled).toBe(true);
		expect(bytesRead).toBe((Math.floor(SERVER_LOG_BODY_MAX_BYTES / chunkBytes) + 1) * chunkBytes);
		expect(await archive(s.deploymentId)).toMatchObject({ log: null, error: "logs HTTP 413 unknown" });
		expect(edgegap.deployment().status).toBe("Status.TERMINATED");
	});

	it("commits the marker before a killed capture and lets the retried stop skip fetching", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		edgegap.hangLogs.add(s.deploymentId);
		const ending = call("POST", `/v1/servers/${s.id}/end`, { reason: "empty" }, { authorization: `Bearer ${s.secret}` });
		try {
			await vi.waitFor(() => expect(edgegap.count("GET", `/v1/deployment/${s.deploymentId}/container-logs`)).toBe(1));
			expect(await archive(s.deploymentId)).toMatchObject({ log: null, error: "capture incomplete" });
			expect(edgegap.count("DELETE", `/v1/stop/${s.deploymentId}`)).toBe(0);
		} finally {
			// Reset the live object while its fetch is pending, preserving its SQLite and D1 state.
			await abortAllDurableObjects();
			await expect(ending).rejects.toThrow("abortAllDurableObjects");
		}
		await age(V, config(env).statusPollMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(edgegap.count("GET", `/v1/deployment/${s.deploymentId}/container-logs`)).toBe(1);
		expect(edgegap.deployment().status).toBe("Status.TERMINATED");
		expect(await archive(s.deploymentId)).toMatchObject({ log: null, error: "capture incomplete" });
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
			expect(await archive(s.deploymentId)).toMatchObject({ log: null, error: `logs HTTP ${failure === "timeout" ? "0 unknown" : failure === "malformed" ? 502 : failure}` });
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

	it("never retries a failed capture when the provider stop needs another attempt", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		edgegap.logStatus = 503;
		edgegap.failStops = 1;
		await end(s);
		const failed = await archive(s.deploymentId);
		edgegap.logStatus = 200;
		await age(V, config(env).statusPollMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(edgegap.count("GET", `/v1/deployment/${s.deploymentId}/container-logs`)).toBe(1);
		expect(await archive(s.deploymentId)).toEqual(failed);
		expect(edgegap.deployment().status).toBe("Status.TERMINATED");
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
		const broken = { ...env, DB: { prepare: () => { throw new Error("D1_ERROR: Network connection lost."); } } as unknown as D1Database };
		await stopWithEvidence(broken, edgegapFor(env, config(env).providerTimeoutMs), "fixture", d.id, config(env).providerTimeoutMs);
		expect(d.status).toBe("Status.TERMINATED");
	});

	it("skips reading logs after a failed marker and records the failure if D1 recovers", async () => {
		const d = edgegap.foreign([], V);
		let refuseMark = true;
		const broken = { ...env, DB: { prepare: (sql: string) => {
			if (refuseMark) {
				refuseMark = false;
				throw new Error("D1_ERROR: D1 DB reset because its code was updated.");
			}
			return env.DB.prepare(sql);
		} } as unknown as D1Database };
		await stopWithEvidence(broken, edgegapFor(env, config(env).providerTimeoutMs), "fixture", d.id, config(env).providerTimeoutMs);
		expect(edgegap.count("GET", `/v1/deployment/${d.id}/container-logs`)).toBe(0);
		expect(await archive(d.id)).toMatchObject({ log: null, error: "evidenceStorage:storage HTTP 0 unknown" });
		expect(d.status).toBe("Status.TERMINATED");
	});

	it("a D1 marker or failure-write stall cannot veto the provider stop", async () => {
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
		for (const [id, at] of [["old", cutoff - 1], ["kept", cutoff]] as const) {
			await env.DB.prepare("INSERT INTO server_crashes VALUES (?1, ?2, 'panic fixture', 0)").bind(id, at).run();
		}
		edgegap.listingDown = true;
		await sweep(env, now);
		expect(await env.DB.prepare("SELECT server_id FROM server_crashes").all()).toMatchObject({ results: [{ server_id: "kept" }] });
		expect(await archive("old")).toBeNull();
		expect(await archive("failed")).toBeNull();
		expect(await archive("kept")).not.toBeNull();
	});
});


describe("archive storage races and failure diagnostics", () => {
	it.each(["marker", "archive"])("an admitted push preserves its archive when %s commits first", async (first) => {
		// D1 serializes these writes in one of these two orders after journalAdmission succeeds.
		// Neither the deadline check nor a real archive may erase the operator's evidence.
		const marker = () => recordMissingJournal(env, "fixture-server", "fixture-deployment", config(env).providerTimeoutMs);
		const push = () => recordJournal(env, "fixture-server", "fixture-deployment", "late diagnostic", true, config(env).providerTimeoutMs);
		if (first === "marker") await marker();
		expect(await push()).toBe(true);
		await marker();
		expect(await archive("fixture-deployment")).toMatchObject({ log: "late diagnostic", truncated: 1, error: null });
		expect(await recordJournal(env, "fixture-server", "fixture-deployment", "replacement", false, config(env).providerTimeoutMs)).toBe(false);
		expect(await archive("fixture-deployment")).toMatchObject({ log: "late diagnostic", truncated: 1, error: null });
	});

	it.each(["journalStorage:storage", "fleet:transport", "history:storage"])("RPC failure cause preserves %s for the caller", async (op) => {
		expect(await evidenceResult("fixture-server", async () => { throw new ProviderError(op, 0, "unknown"); }))
			.toEqual({ ok: false, cause: op });
	});
});

describe("bounded archive redaction", () => {
	it.each(redactionCases)("preserves diagnostics and masks credentials in $raw for stored evidence", ({ raw, expected }) => {
		expect(sanitizedTail(raw, "").log).toBe(expected);
	});

	it("configuration names and netcode diagnostics survive value redaction for the operator", () => {
		// hosting.rs ConfigError::Display and the vendored server.rs whitelist info! shape.
		const configError = "fatal: OVERMATCH_CAPACITY: expected an integer from 1 to 256";
		const netcode = "server ignored connection request. server address not in connect token whitelist token_addresses=[192.0.2.1:5888] server_addr=192.0.2.2:5888";
		const { log } = sanitizedTail([configError, netcode, "Netcode error: InvalidToken(InvalidVersion)", "request token=opaque-value rejected: expired", "OVERMATCH_API=value"].join("\n"), "");
		expect(log).toContain(configError);
		expect(log).toContain("Netcode error: InvalidToken(InvalidVersion)");
		expect(log).toContain("server address not in connect token whitelist token_addresses=[[ip]:5888]");
		expect(log).toContain("request token=[credential] rejected: expired");
		expect(log).toContain("OVERMATCH_API=[credential]");
		expect(log).not.toContain("opaque-value");
	});

	it("the complete byte-cap adversary is processed within a generous linear-time budget", async () => {
		// Resolution: DERIVED cap / 256 KiB per second; slow CI may spend 4 s at 1 MiB,
		// but the former quadratic scan takes minutes and must fail this gate.
		const boundMs = SERVER_LOG_MAX_BYTES / (256 * 1024) * 1000;
		const raw = "a-".repeat(SERVER_LOG_MAX_BYTES / 2);
		// Worker clocks advance at I/O; yield before and after CPU work to measure wall time.
		await new Promise((resolve) => setTimeout(resolve, 0));
		const start = performance.now();
		expect(sanitizedTail(raw, "").log).toBe(raw);
		await new Promise((resolve) => setTimeout(resolve, 0));
		const elapsed = performance.now() - start;
		console.warn(`Worker sanitiser at ${SERVER_LOG_MAX_BYTES} bytes: ${elapsed.toFixed(1)} ms (Resolution bound ${boundMs} ms)`);
		expect(elapsed).toBeLessThan(boundMs);
	});

	it.each(["declared", "chunked"])("end crash input is refused at the stream byte cap (%s)", async (kind) => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		const s = await bringUp();
		let reads = 0;
		const body = new ReadableStream<Uint8Array>({ pull(c) { reads++; c.enqueue(new Uint8Array(SERVER_LOG_MAX_BYTES / 2 + 1).fill(120)); } }, { highWaterMark: 0 });
		const response = await (await import("../src/index")).default.fetch(new Request(`https://api.overmatch.test/v1/servers/${s.id}/end`, {
			method: "POST", body, headers: { authorization: `Bearer ${s.secret}`, ...(kind === "declared" ? { "content-length": String(SERVER_LOG_MAX_BYTES + 1) } : {}) },
		}), env);
		expect(response.status).toBe(413);
		expect(reads).toBe(kind === "declared" ? 0 : 2);
		expect((await registry(V).snapshot())[0].state).toBe("ready");
		expect(await env.DB.prepare("SELECT * FROM server_crashes WHERE server_id = ?1").bind(s.id).first()).toBeNull();
	});
});
