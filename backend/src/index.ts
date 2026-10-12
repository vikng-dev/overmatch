// HTTP entry point: validates requests, routes each to its version's Registry, maps results to
// status codes. No state lives here. The contract is src/types.ts + contract/*.json.

import { accessDenied, adminPage } from "./admin";
import type { Env } from "./config";
import { validVersion, versionOf } from "./ids";
import { diag } from "./reasons";
import { registryFor } from "./registry";
import type { HeartbeatRequest, JoinRequest } from "./types";

export { Registry } from "./registry";

const MAX_LOCATIONS = 64;

function json(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

function error(status: number, message: string): Response {
	return json({ error: message }, status);
}

async function body(req: Request): Promise<unknown> {
	try {
		return await req.json();
	} catch {
		return undefined;
	}
}

function bearer(req: Request): string {
	const h = req.headers.get("authorization") ?? "";
	return h.startsWith("Bearer ") ? h.slice(7).trim() : "";
}

function parseJoin(b: unknown): JoinRequest | string {
	if (typeof b !== "object" || b === null) return "body must be a JSON object";
	const { version, locations, pings } = b as Record<string, unknown>;
	if (!validVersion(version)) return "version: expected 1-64 chars of [A-Za-z0-9._+-]";
	if (!Array.isArray(locations) || locations.length === 0 || locations.length > MAX_LOCATIONS) {
		return `locations: expected 1-${MAX_LOCATIONS} location ids`;
	}
	if (!locations.every((l) => typeof l === "string" && l.length > 0 && l.length <= 128)) return "locations: expected strings";
	const p = pings ?? {};
	if (typeof p !== "object" || p === null || Array.isArray(p)) return "pings: expected an object of location id to ms";
	const out: Record<string, number> = {};
	for (const [k, v] of Object.entries(p)) {
		if (typeof v !== "number" || !Number.isFinite(v) || v < 0) return `pings.${k}: expected a non-negative number`;
		out[k] = v;
	}
	return { version, locations: [...new Set(locations as string[])], pings: out };
}

function parseHeartbeat(b: unknown): HeartbeatRequest | string {
	if (typeof b !== "object" || b === null) return "body must be a JSON object";
	const { players, capacity, state } = b as Record<string, unknown>;
	if (!Number.isInteger(players) || (players as number) < 0) return "players: expected a non-negative integer";
	if (!Number.isInteger(capacity) || (capacity as number) < 1) return "capacity: expected a positive integer";
	if (state !== "open" && state !== "closing") return 'state: expected "open" or "closing"';
	return { players: players as number, capacity: capacity as number, state };
}

async function route(req: Request, env: Env): Promise<Response> {
	const url = new URL(req.url);
	const path = url.pathname.replace(/\/+$/, "") || "/";
	const method = req.method;

	if (path === "/v1/locations" && method === "GET") {
		const version = url.searchParams.get("version");
		if (!validVersion(version)) return error(400, "version query parameter required");
		const r = await registryFor(env, version).locations(version);
		return r.ok ? json(r.body) : error(503, r.error);
	}

	if (path === "/v1/join" && method === "POST") {
		const j = parseJoin(await body(req));
		if (typeof j === "string") return error(400, j);
		const ip = req.headers.get("cf-connecting-ip");
		const r = await registryFor(env, j.version).join(j.version, j.locations, j.pings, ip, url.origin);
		return r.ok ? json({ ticket: r.ticket }) : error(r.status, r.error);
	}

	// The ticket is a bearer credential: it travels in the body, never in the URL.
	if (path === "/v1/join/status" && method === "POST") {
		const b = await body(req);
		const ticket = typeof b === "object" && b !== null ? (b as { ticket?: unknown }).ticket : undefined;
		const version = typeof ticket === "string" ? versionOf(ticket) : null;
		if (typeof ticket !== "string" || !version) return error(404, "unknown ticket");
		const t = await registryFor(env, version).ticket(ticket);
		return t ? json(t) : error(404, "unknown ticket");
	}

	let m = /^\/v1\/servers\/([^/]+)\/(heartbeat|end)$/.exec(path);
	if (m && method === "POST") {
		const id = decodeURIComponent(m[1]);
		const version = versionOf(id);
		if (!version) return error(410, "unknown server");
		const reg = registryFor(env, version);
		const b = await body(req);
		let r;
		if (m[2] === "heartbeat") {
			const hb = parseHeartbeat(b);
			if (typeof hb === "string") return error(400, hb);
			r = await reg.heartbeat(id, bearer(req), hb);
		} else {
			const reason = typeof b === "object" && b !== null && typeof (b as { reason?: unknown }).reason === "string" ? (b as { reason: string }).reason : "exit";
			r = await reg.end(id, bearer(req), reason.slice(0, 200));
		}
		if (r.status === 204) return new Response(null, { status: 204 });
		return error(r.status, r.error ?? (r.status === 410 ? "unknown server" : "error"));
	}

	// Edgegap's webhook_url takes no headers, so the path names the server and carries its own nonce.
	// It authorizes nothing but a status re-read for that one server, so a logged URL is no key.
	m = /^\/v1\/edgegap\/webhook\/([^/]+)\/([0-9a-f]{32})$/.exec(path);
	if (m && method === "POST") {
		const id = decodeURIComponent(m[1]);
		const version = versionOf(id);
		const hook = await body(req);
		if (!version) return error(404, "unknown webhook");
		const rid = typeof hook === "object" && hook !== null ? (hook as { request_id?: unknown }).request_id : undefined;
		const requestId = typeof rid === "string" && /^[A-Za-z0-9-]{1,64}$/.test(rid) ? rid : null;
		const known = await registryFor(env, version).webhook(id, m[2], requestId);
		return known ? new Response(null, { status: 204 }) : error(404, "unknown webhook");
	}

	if (path === "/admin" && method === "GET") {
		const denied = await accessDenied(req, env);
		if (denied) return new Response(denied, { status: 403, headers: { "content-type": "text/plain" } });
		return new Response(await adminPage(env), { headers: { "content-type": "text/html; charset=utf-8" } });
	}

	return error(404, "not found");
}

export default {
	async fetch(req: Request, env: Env): Promise<Response> {
		try {
			return await route(req, env);
		} catch (err) {
			console.error("unhandled", diag(err));
			return error(500, "internal error");
		}
	},
} satisfies ExportedHandler<Env>;
