// HTTP entry point: validates requests, routes each to its version's Registry, maps results to
// status codes. No state lives here. The contract is src/types.ts + contract/*.json.

import { accessDenied, adminPage } from "./admin";
import { config, ConfigurationError, SERVER_SETTING_DOMAINS, serverEnvironment, type Env } from "./config";
import { newId, randomHex, validVersion, versionOf, tagOf } from "./ids";
import { diag } from "./reasons";
import { ProviderError, providerFor, platformCall, isRecord } from "./provider";
import { registryFor, type JournalResult } from "./registry";
import { SERVER_LOG_MAX_BYTES } from "./evidence";
import { sweep } from "./sweep";
import type { HeartbeatRequest, JoinRequest } from "./types";

export { Fleet } from "./fleet";
export { Registry } from "./registry";

const MAX_LOCATIONS = 64;
// src/net/matchmaking/contract.rs:51: client RTTs are whole milliseconds stored as u32.
const MAX_PING_MS = 2 ** 32 - 1;

function json(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

function error(status: number, message: string): Response {
	return json({ error: message }, status);
}

class RequestError extends Error {
	constructor(readonly kind: "bodyRead" | "json" | "pathEncoding" | "fields" | "clientIp" | "journalSize" | "journalEncoding", message: string, readonly status = 400) {
		super(message);
		this.name = "RequestError";
	}
}

function invalid(message: string): never { throw new RequestError("fields", message); }

async function body(req: Request): Promise<unknown> {
	if (req.bodyUsed || req.body?.locked) throw new Error("request body must be read once at the route edge");
	let raw: string;
	try { raw = await req.text(); }
	catch {
		// Only the native request stream runs here; disconnected streams have no stable error subtype.
		throw new RequestError("bodyRead", "request body could not be read");
	}
	try { return JSON.parse(raw); }
	catch (err) {
		if (!(err instanceof SyntaxError)) throw err;
		throw new RequestError("json", "body must be a JSON object");
	}
}

/** Stream admission bounds bytes before decoding/concatenation; Content-Length can only refuse early. */
async function journalBody(req: Request): Promise<string> {
	const declared = req.headers.get("content-length");
	if (declared !== null && (!/^[0-9]+$/.test(declared) || Number(declared) > SERVER_LOG_MAX_BYTES)) {
		throw new RequestError("journalSize", "journal exceeds byte cap", 413);
	}
	if (!req.body) return "";
	const reader = req.body.getReader();
	const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false });
	let size = 0, raw = "";
	try {
		while (true) {
			let chunk: ReadableStreamReadResult<Uint8Array>;
			try { chunk = await reader.read(); }
			catch {
				// Native request streams have no stable platform failure subtype.
				throw new RequestError("bodyRead", "journal could not be read");
			}
			if (chunk.done) break;
			size += chunk.value.byteLength;
			if (size > SERVER_LOG_MAX_BYTES) throw new RequestError("journalSize", "journal exceeds byte cap", 413);
			try { raw += decoder.decode(chunk.value, { stream: true }); }
			catch (err) {
				if (!(err instanceof TypeError)) throw err;
				throw new RequestError("journalEncoding", "journal must be UTF-8");
			}
		}
		try { return raw + decoder.decode(); }
		catch (err) {
			if (!(err instanceof TypeError)) throw err;
			throw new RequestError("journalEncoding", "journal must be UTF-8");
		}
	} finally {
		// Cancellation must not wait for an untrusted producer to acknowledge it.
		void reader.cancel().catch(() => console.warn("journal stream cancellation failed"));
		reader.releaseLock();
	}
}

function pathSegment(raw: string): string {
	try { return decodeURIComponent(raw); }
	catch (err) {
		if (!(err instanceof URIError)) throw err;
		throw new RequestError("pathEncoding", "invalid path encoding");
	}
}

function bearer(req: Request): string {
	const h = req.headers.get("authorization") ?? "";
	return h.startsWith("Bearer ") ? h.slice(7).trim() : "";
}

function clientIp(req: Request): string | null {
	const raw = req.headers.get("cf-connecting-ip");
	// Local/test requests may have no Cloudflare client address.
	if (raw === null) return null;
	if (/^(?:[0-9]{1,3}\.){3}[0-9]{1,3}$/.test(raw) && raw.split(".").every((part) => Number(part) <= 255)) return raw;
	if (raw.includes(":") && /^[0-9a-f:.]+$/i.test(raw)) {
		try { new URL(`http://[${raw}]/`); return raw; }
		catch (err) { if (!(err instanceof TypeError)) throw err; }
	}
	throw new RequestError("clientIp", "invalid cf-connecting-ip header");
}

function parseJoin(b: unknown): JoinRequest {
	if (!isRecord(b)) invalid("body must be a JSON object");
	const { version, locations, pings } = b;
	if (!validVersion(version)) invalid("version: expected 1-64 chars of [A-Za-z0-9._+-]");
	if (!Array.isArray(locations) || locations.length === 0 || locations.length > MAX_LOCATIONS) {
		invalid(`locations: expected 1-${MAX_LOCATIONS} location ids`);
	}
	const locationId = (l: unknown): l is string => typeof l === "string" && l.isWellFormed() && l.length > 0 && l.length <= 128;
	if (!locations.every(locationId)) invalid("locations: expected 1-128 character Unicode location ids");
	const p = pings === undefined ? {} : pings;
	if (typeof p !== "object" || p === null || Array.isArray(p)) invalid("pings: expected an object of location id to ms");
	if (Object.keys(p).length > MAX_LOCATIONS) invalid(`pings: expected at most ${MAX_LOCATIONS} locations`);
	const entries: [string, number][] = [];
	for (const [k, v] of Object.entries(p)) {
		if (!locationId(k)) invalid("pings: expected 1-128 character Unicode location ids");
		if (typeof v !== "number" || !Number.isInteger(v) || v < 0 || v > MAX_PING_MS) invalid(`pings: expected integer ms from 0 to ${MAX_PING_MS}`);
		entries.push([k, v]);
	}
	return { version, locations: [...new Set(locations as string[])], pings: Object.fromEntries(entries) };
}

function parseHeartbeat(b: unknown): HeartbeatRequest {
	if (!isRecord(b)) invalid("body must be a JSON object");
	const { players, capacity, state } = b;
	// Counts fit the hosted netcode ceiling; SQLite LIMIT receives only 0..capacity arrivals.
	const domain = SERVER_SETTING_DOMAINS.CAPACITY;
	if (typeof capacity !== "number" || !Number.isInteger(capacity) || capacity < domain.min || capacity > domain.max) invalid(`capacity: expected an integer from ${domain.min} to ${domain.max}`);
	if (typeof players !== "number" || !Number.isInteger(players) || players < 0 || players > capacity) invalid("players: expected an integer from 0 to capacity");
	if (state !== "open" && state !== "closed" && state !== "closing") invalid('state: expected "open", "closed" or "closing"');
	return { players, capacity, state };
}

async function route(req: Request, env: Env): Promise<Response> {
	const cfg = config(env);
	const url = new URL(req.url);
	const path = url.pathname.replace(/\/+$/, "") || "/";
	const method = req.method;

	if (path === "/v1/locations" && method === "GET") {
		const version = url.searchParams.get("version");
		if (!validVersion(version)) invalid("version query parameter required");
		const r = await platformCall("locations", async () => registryFor(env, version).locations(version));
		return r.ok ? json(r.body) : error(503, r.error);
	}

	if (path === "/v1/join" && method === "POST") {
		const j = parseJoin(await body(req));
		const ip = clientIp(req);
		const r = await platformCall("join", async () => registryFor(env, j.version).join(j.version, j.locations, j.pings, ip, url.origin));
		return r.ok ? json({ ticket: r.ticket }) : error(r.status, r.error);
	}

	// The ticket is a bearer credential: it travels in the body, never in the URL.
	if (path === "/v1/join/status" && method === "POST") {
		const b = await body(req);
		const ticket = typeof b === "object" && b !== null ? (b as { ticket?: unknown }).ticket : undefined;
		const version = typeof ticket === "string" ? versionOf(ticket) : null;
		if (typeof ticket !== "string" || !version) return error(404, "unknown ticket");
		const t = await platformCall("ticket", async () => registryFor(env, version).ticket(ticket));
		return t ? json(t) : error(404, "unknown ticket");
	}

	const journal = /^\/v1\/servers\/([^/]+)\/journal$/.exec(path);
	if (journal && method === "POST") {
		const id = pathSegment(journal[1]);
		const version = versionOf(id);
		const refuse = (status: number, cause: string) => {
			console.warn("journal refused", status, cause);
			return error(status, cause);
		};
		if (!version) return refuse(410, "unknown server");
		const secret = bearer(req);
		const registry = registryFor(env, version);
		const admission = await platformCall<JournalResult>("journalAdmission", () => registry.journalAdmission(id, secret));
		if (admission.status !== 204) return refuse(admission.status, admission.error);
		const truncated = req.headers.get("x-journal-truncated");
		if (truncated !== "0" && truncated !== "1") return refuse(400, "journal truncation flag required");
		const raw = await journalBody(req);
		const saved = await platformCall<JournalResult>("journal", () => registry.journal(id, secret, raw, truncated === "1"));
		return saved.status === 204 ? new Response(null, { status: 204 }) : refuse(saved.status, saved.error);
	}

	let m = /^\/v1\/servers\/([^/]+)\/(heartbeat|end)$/.exec(path);
	if (m && method === "POST") {
		const id = pathSegment(m[1]);
		const version = versionOf(id);
		if (!version) return error(410, "unknown server");
		const b = await body(req);
		let r;
		if (m[2] === "heartbeat") {
			const hb = parseHeartbeat(b);
			const secret = bearer(req);
			r = await platformCall("heartbeat", async () => registryFor(env, version).heartbeat(id, secret, hb));
		} else {
			if (!isRecord(b)) invalid("body must be a JSON object");
			// Diagnostics accept arbitrary Unicode text, including empty text; history keeps a short
			// reason and evidence.ts owns redaction and the bounded UTF-8 crash tail.
			if (b.reason !== undefined && (typeof b.reason !== "string" || !b.reason.isWellFormed())) invalid("reason: expected Unicode text");
			if (b.crash !== undefined && (typeof b.crash !== "string" || !b.crash.isWellFormed())) invalid("crash: expected Unicode text");
			// An omitted reason is the protocol's ordinary exit report.
			const reason = b.reason === undefined ? "exit" : b.reason;
			const crash = b.crash;
			const secret = bearer(req);
			r = await platformCall("end", async () => registryFor(env, version).end(id, secret, reason.slice(0, 200), crash));
		}
		if (r.status === 204) return new Response(null, { status: 204 });
		return error(r.status, r.error ?? (r.status === 410 ? "unknown server" : "error"));
	}

	// Edgegap's webhook_url takes no headers, so the path names the server and carries its own nonce.
	// It authorizes nothing but a status re-read for that one server, so a logged URL is no key.
	m = /^\/v1\/edgegap\/webhook\/([^/]+)\/([0-9a-f]{32})$/.exec(path);
	if (m && method === "POST") {
		const id = pathSegment(m[1]);
		const version = versionOf(id);
		const hook = await body(req);
		if (!version) return error(404, "unknown webhook");
		if (!isRecord(hook)) invalid("body must be a JSON object");
		const rid = hook.request_id;
		if (rid !== undefined && (typeof rid !== "string" || /^[A-Za-z0-9-]{1,64}$/.exec(rid)?.[0] !== rid)) invalid("request_id: expected 1-64 letters, digits or hyphens");
		const requestId = rid === undefined ? null : rid;
		const nonce = m[2];
		const known = await platformCall("webhook", async () => registryFor(env, version).webhook(id, nonce, requestId));
		return known ? new Response(null, { status: 204 }) : error(404, "unknown webhook");
	}

	// Cutover verification uses this Access-protected tool; dryRun creates no machine or fleet slot.
	if (path === "/admin/aws-proof" && method === "POST") {
		const provider = providerFor(env, cfg.providerTimeoutMs);
		if (!provider.dryRun || !provider.verifyRelease) return error(404, "provider has no launch proof");
		const denied = await accessDenied(req, env);
		if (denied) return error(403, denied);
		const b = await body(req);
		if (!isRecord(b) || !validVersion(b.version)) invalid("version required");
		const image = "imageId" in b ? b.imageId : undefined;
		if (image !== undefined && (typeof image !== "string" || /^ami-[0-9a-f]{17}$/.exec(image)?.[0] !== image)) invalid("invalid proof image");
		const id = newId(b.version);
		try {
			const [location] = await provider.locations();
			if (!location) throw new Error("launch proof requires at least one provider location");
			await provider.dryRun({ version: b.version, city: location.city, tag: tagOf(id), clientIp: null, webhookUrl: "",
				release: await provider.verifyRelease(b.version, cfg.providerTimeoutMs), env: serverEnvironment(cfg, id, url.origin, randomHex(32), randomHex(32)) }, image);
			return json({ proof: "authorized", image: image === undefined ? "public-parameter" : "concrete", creates: false });
		} catch (err) {
			if (!(err instanceof ProviderError)) throw err;
			console.warn("AWS proof refused", diag(err));
			return error(503, "AWS launch authorization not confirmed; inspect the failure kind in Worker logs");
		}
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
			config(env);
			return await route(req, env);
		} catch (err) {
			if (err instanceof ConfigurationError) {
				console.error("configuration refused", err.message);
				return error(503, err.message);
			}
			if (err instanceof RequestError) {
				console.warn("request refused", err.kind);
				return error(err.status, err.message);
			}
			if (!(err instanceof ProviderError)) throw err;
			console.warn("platform unavailable", diag(err));
			return error(503, "service unavailable; retry later");
		}
	},

	// The orphan sweep (src/sweep.ts), on the `triggers.crons` schedule in wrangler.jsonc.
	async scheduled(_controller: ScheduledController, env: Env): Promise<void> {
		try { config(env); }
		catch (err) {
			if (!(err instanceof ConfigurationError)) throw err;
			console.error("configuration refused", err.message);
			return;
		}
		await sweep(env);
	},
} satisfies ExportedHandler<Env>;
