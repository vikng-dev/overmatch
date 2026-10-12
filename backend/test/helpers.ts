import { runInDurableObject } from "cloudflare:test";
import type { D1Migration } from "cloudflare:test";
import { env as providedEnv, exports } from "cloudflare:workers";
import type { Env } from "../src/config";
import type { Registry } from "../src/registry";
import type { HeartbeatRequest, JoinRequest, TicketResponse } from "../src/types";

export const ORIGIN = "https://api.overmatch.test";

/** The Worker's env as the code under test declares it (src/config.ts), plus the test-only
 *  migrations binding from vitest.config.ts. */
export const env = providedEnv as unknown as Env & { TEST_MIGRATIONS: D1Migration[] };

export async function call(method: string, path: string, body?: unknown, headers: Record<string, string> = {}): Promise<Response> {
	return exports.default.fetch(`${ORIGIN}${path}`, {
		method,
		headers: { "content-type": "application/json", ...headers },
		body: body === undefined ? undefined : JSON.stringify(body),
	});
}

export async function join(req: JoinRequest): Promise<string> {
	const res = await call("POST", "/v1/join", req);
	const out = (await res.json()) as { ticket?: string; error?: string };
	if (res.status !== 200 || !out.ticket) throw new Error(`join ${res.status}: ${JSON.stringify(out)}`);
	return out.ticket;
}

export async function ticket(t: string): Promise<TicketResponse> {
	const res = await call("GET", `/v1/join/${t}`);
	if (res.status !== 200) throw new Error(`ticket ${res.status}: ${await res.text()}`);
	return (await res.json()) as TicketResponse;
}

export function heartbeat(serverId: string, secret: string, hb: HeartbeatRequest): Promise<Response> {
	return call("POST", `/v1/servers/${serverId}/heartbeat`, hb, { authorization: `Bearer ${secret}` });
}

export function webhook(serverId: string, token = "test-webhook-secret", requestId = "ignored"): Promise<Response> {
	return call("POST", `/v1/edgegap/webhook?server=${encodeURIComponent(serverId)}&token=${encodeURIComponent(token)}`, { request_id: requestId });
}

export function registry(version: string): DurableObjectStub<Registry> {
	return env.REGISTRY.getByName(version);
}

/** Move every stored time of `version`'s registry back by `ms`, so deadlines pass without a clock. */
export async function age(version: string, ms: number): Promise<void> {
	await runInDurableObject(registry(version), (_: Registry, state) => {
		state.storage.sql.exec(
			`UPDATE servers SET created_at = created_at - ?1, last_seen = last_seen - ?1, next_poll_at = next_poll_at - ?1,
			   ready_at = ready_at - ?1, ended_at = ended_at - ?1, stop_at = stop_at - ?1`,
			ms,
		);
		state.storage.sql.exec("UPDATE tickets SET created_at = created_at - ?1, attached_at = attached_at - ?1", ms);
	});
}

/** A server's id, report secret and netcode key, as Edgegap received them in the deploy call. */
export function launched(n = 0): { id: string; secret: string; key: string; deploymentId: string } {
	const d = edgegap.deployment(n);
	return {
		id: edgegap.env(d, "OVERMATCH_SERVER_ID"),
		secret: edgegap.env(d, "OVERMATCH_REPORT_SECRET"),
		key: edgegap.env(d, "OVERMATCH_NETCODE_KEY"),
		deploymentId: d.id,
	};
}

/** Drive server n to joinable: Edgegap ready + ready webhook + first heartbeat. */
export async function bringUp(n = 0, players = 0): Promise<{ id: string; secret: string; key: string; deploymentId: string }> {
	const s = launched(n);
	edgegap.ready(edgegap.deployment(n));
	expectStatus(await webhook(s.id), 204);
	expectStatus(await heartbeat(s.id, s.secret, { players, capacity: 10, state: "open" }), 204);
	return s;
}

export function expectStatus(res: Response, status: number): void {
	if (res.status !== status) throw new Error(`expected HTTP ${status}, got ${res.status}`);
}
