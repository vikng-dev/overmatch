// Operator tunables are wrangler `vars` entries; production-server.json owns the production CPU
// allocation (CLAIMED: owner's hosting choice), shared with scripts/ci/edgegap-version.sh.

import productionServer from "../production-server.json";
import { parseProviderName } from "./provider";

const vcpuPerServer = productionServer.vcpu;
if (!Number.isFinite(vcpuPerServer) || vcpuPerServer <= 0) {
	throw new Error(`config: production-server.json vcpu=${String(vcpuPerServer)} (need a finite number > 0)`);
}

export interface Env {
	// aws-proving uses EC2; production stays Edgegap until cutover.
	PROVIDER: string;
	// Operator-owned immutable identity; every EC2 deployment needs a distinct value.
	WORKER_ENV: string;
	FLEET: DurableObjectNamespace<import("./fleet").Fleet>;
	AWS_ACCESS_KEY_ID?: string;
	AWS_SECRET_ACCESS_KEY?: string;
	AWS_HOST_CONFIG?: string;
	REGISTRY: DurableObjectNamespace<import("./registry").Registry>;
	DB: D1Database;
	EDGEGAP_API: string;
	EDGEGAP_APP: string;
	EDGEGAP_PORT_NAME: string;
	EDGEGAP_TOKEN: string;
	CAPACITY: string;
	HEARTBEAT_INTERVAL_S: string;
	HEARTBEAT_DEADLINE_S: string;
	START_DEADLINE_S: string;
	STATUS_POLL_S: string;
	RESERVATION_TTL_S: string;
	TICKET_TTL_S: string;
	EMPTY_EXIT_S: string;
	SERVER_STATS_S: string;
	SERVER_LOG_RETENTION_DAYS: string;
	ENDED_RETENTION_S: string;
	JOIN_ATTEMPTS?: string;
	LOCATIONS_CACHE_S: string;
	PROVIDER_TIMEOUT_S: string;
	USD_PER_VCPU_MINUTE?: string;
	LOCATION_HINT: string;
	ACCESS_TEAM_DOMAIN?: string;
	ACCESS_AUD?: string;
}

export class ConfigurationError extends Error {
	constructor(readonly setting: keyof Env, reason: string) {
		super(`config: ${setting}: ${reason}`);
		this.name = "ConfigurationError";
	}
}

export interface Network { securityGroupId: string; subnetIds: string[] }
export type ProviderConfig =
	| { name: "edgegap"; api: URL; headers: Headers; app: string; portName: string; attempts: number; usdPerVcpuMinute: number }
	| { name: "ec2"; workerEnv: string; accessKey: string | null; secretKey: string | null; network: Network | null };

export interface Config {
	provider: ProviderConfig;
	access: { issuer: string; keys: URL; audience: string } | null;
	locationHint: DurableObjectLocationHint | "";
	capacity: number;
	heartbeatIntervalMs: number;
	heartbeatDeadlineMs: number;
	startDeadlineMs: number;
	statusPollMs: number;
	reservationTtlMs: number;
	ticketTtlMs: number;
	emptyExitS: number;
	serverStatsS: number;
	serverLogRetentionMs: number;
	endedRetentionMs: number;
	locationsCacheMs: number;
	providerTimeoutMs: number;
	vcpuPerServer: number;
}

// src/net/hosting.rs owns the server's domains; the Worker mirrors them before launch.
// Rust and TypeScript have no shared bounds source. Existing Worker minima stay stricter where noted.
export const SERVER_SETTING_DOMAINS = {
	CAPACITY: { min: 1, max: 256, integer: true }, // hosting.rs:126,219 (netcode ceiling)
	HEARTBEAT_INTERVAL_S: { min: 1, max: 86_400 }, // hosting.rs:289-292; Worker minimum: 1 s
	EMPTY_EXIT_S: { min: 0, exclusiveMin: true, max: 86_400 }, // hosting.rs:289-292
	// Unknown: no operating-frequency or storage budget establishes the Worker's 1 ms minimum.
	SERVER_STATS_S: { min: 0.001, max: 2 ** 64, exclusiveMax: true }, // server_stats.rs:295-312
} satisfies Record<string, { min: number; max: number; integer?: boolean; exclusiveMin?: boolean; exclusiveMax?: boolean }>;

function num(env: Env, key: keyof Env, min: number, scale = 1): number {
	const raw = env[key];
	const v = typeof raw === "string" && /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.exec(raw.trim())?.[0] === raw.trim() ? Number(raw) : NaN;
	if (!Number.isFinite(v) || v < min || !Number.isFinite(v * scale)) {
		throw new ConfigurationError(key, `need a finite number >= ${min} with a finite converted value`);
	}
	return v * scale;
}

// Alarm timestamps and SQLite deadlines must retain millisecond precision when added to now.
// The provider's shorter bound is the signed 32-bit delay accepted by setTimeout.
function milliseconds(env: Env, key: keyof Env, min: number, scale: number, max = Number.MAX_SAFE_INTEGER - Date.now()): number {
	const value = num(env, key, min, scale);
	if (value > max) throw new ConfigurationError(key, `converted duration must be <= ${max} ms; reduce this setting in wrangler vars`);
	return value;
}

function serverNumber(env: Env, key: keyof typeof SERVER_SETTING_DOMAINS, scale = 1): number {
	const domain = SERVER_SETTING_DOMAINS[key];
	const value = num(env, key, domain.min);
	if (value > domain.max || ("exclusiveMax" in domain && value === domain.max)
		|| ("exclusiveMin" in domain && value === domain.min) || ("integer" in domain && !Number.isInteger(value))) {
		throw new ConfigurationError(key, `need ${"integer" in domain ? "an integer" : "seconds"} ${"exclusiveMin" in domain ? ">" : ">="} ${domain.min} and ${"exclusiveMax" in domain ? "<" : "<="} ${domain.max}`);
	}
	return value * scale;
}

function optionalText(env: Env, key: keyof Env): string | null {
	const raw = env[key];
	if (raw === undefined) return null;
	if (typeof raw !== "string" || raw.trim() === "" || !raw.isWellFormed() || /[\u0000-\u001f\u007f]/.test(raw)) {
		throw new ConfigurationError(key, "need nonempty text without control characters (unset is absent)");
	}
	return raw;
}

function requiredText(env: Env, key: keyof Env): string {
	const value = optionalText(env, key);
	if (value === null) throw new ConfigurationError(key, "need nonempty text");
	return value;
}

function optionalToken(env: Env, key: keyof Env): string | null {
	const value = optionalText(env, key);
	if (value !== null && /[^\x21-\x7e]/.test(value)) throw new ConfigurationError(key, "need a visible ASCII token without spaces (unset is absent)");
	return value;
}

function settingUrl(raw: string, key: keyof Env): URL {
	if (typeof raw !== "string" || !raw.isWellFormed() || /\s|[\u0000-\u001f\u007f]/.test(raw)) throw new ConfigurationError(key, "need an HTTP(S) URL");
	let url: URL;
	try { url = new URL(raw); }
	catch (err) {
		if (!(err instanceof TypeError)) throw err;
		throw new ConfigurationError(key, "need an HTTP(S) URL");
	}
	if (!["https:", "http:"].includes(url.protocol) || url.username || url.password) {
		throw new ConfigurationError(key, "need an HTTP(S) URL without credentials");
	}
	return url;
}

function header(value: string, key: keyof Env): Headers {
	try { return new Headers({ authorization: value }); }
	catch (err) {
		if (!(err instanceof TypeError)) throw err;
		throw new ConfigurationError(key, "need a valid HTTP header value");
	}
}

function network(raw: string | undefined): Network | null {
	// A keyless Worker is deployable; launch refuses a missing network before reserving a slot.
	if (raw === undefined) return null;
	if (typeof raw !== "string") throw new ConfigurationError("AWS_HOST_CONFIG", "need a JSON object with securityGroupId and unique subnetIds");
	let n: unknown;
	try { n = JSON.parse(raw); }
	catch (err) {
		if (!(err instanceof SyntaxError)) throw err;
		throw new ConfigurationError("AWS_HOST_CONFIG", "need a JSON object with securityGroupId and unique subnetIds");
	}
	if (n && typeof n === "object" && "securityGroupId" in n && "subnetIds" in n
		&& Object.keys(n).length === 2 && typeof n.securityGroupId === "string" && /^sg-[0-9a-f]{17}$/.exec(n.securityGroupId)?.[0] === n.securityGroupId
		&& Array.isArray(n.subnetIds) && n.subnetIds.length > 0
		&& n.subnetIds.every((s: unknown) => typeof s === "string" && /^subnet-[0-9a-f]{17}$/.exec(s)?.[0] === s)
		&& new Set(n.subnetIds).size === n.subnetIds.length) return n as Network;
	throw new ConfigurationError("AWS_HOST_CONFIG", "need a JSON object with securityGroupId and unique subnetIds");
}

function providerConfig(env: Env): ProviderConfig {
	const name = parseProviderName(env.PROVIDER);
	if (name === "edgegap") {
		const api = settingUrl(env.EDGEGAP_API, "EDGEGAP_API");
		const token = requiredText(env, "EDGEGAP_TOKEN");
		const headers = header(token.startsWith("token ") ? token : `token ${token}`, "EDGEGAP_TOKEN");
		headers.set("content-type", "application/json");
		headers.set("accept", "application/json");
		return { name, api, headers, app: requiredText(env, "EDGEGAP_APP"), portName: requiredText(env, "EDGEGAP_PORT_NAME"),
			attempts: num(env, "JOIN_ATTEMPTS", 1), usdPerVcpuMinute: num(env, "USD_PER_VCPU_MINUTE", 0) };
	}
	const workerEnv = env.WORKER_ENV;
	if (typeof workerEnv !== "string" || /^[a-z][a-z0-9-]{0,62}$/.exec(workerEnv)?.[0] !== workerEnv) {
		throw new ConfigurationError("WORKER_ENV", "need this deployment's lowercase letters, digits and hyphens");
	}
	const accessKey = optionalToken(env, "AWS_ACCESS_KEY_ID");
	return { name, workerEnv, accessKey, secretKey: optionalToken(env, "AWS_SECRET_ACCESS_KEY"), network: network(env.AWS_HOST_CONFIG) };
}

const configurations = new WeakMap<Env, Config>();

/** HTTP/scheduled entry parses before work; DO construction does so in its own isolate.
 * Worker bindings are immutable: all downstream consumers reuse this checked configuration. */
export function config(env: Env): Config {
	const cached = configurations.get(env);
	if (cached) return cached;
	const provider = providerConfig(env);
	const team = optionalText(env, "ACCESS_TEAM_DOMAIN");
	const teamUrl = team === null ? null : settingUrl(team, "ACCESS_TEAM_DOMAIN");
	if (teamUrl && (!/^\/*$/.test(teamUrl.pathname) || teamUrl.search || teamUrl.hash)) {
		throw new ConfigurationError("ACCESS_TEAM_DOMAIN", "need an HTTP(S) origin without a path, query or fragment");
	}
	const issuer = teamUrl?.origin ?? null;
	const audience = optionalText(env, "ACCESS_AUD");
	const access = issuer && audience ? { issuer, keys: new URL(`${issuer}/cdn-cgi/access/certs`), audience } : null;
	const hint = env.LOCATION_HINT;
	if (!["", "wnam", "enam", "sam", "weur", "eeur", "apac", "apac-ne", "apac-se", "oc", "afr", "me"].includes(hint)) {
		throw new ConfigurationError("LOCATION_HINT", "need a Cloudflare Durable Object location hint or an empty string");
	}
	const s = 1000;
	const cfg: Config = {
		provider, access, locationHint: hint as DurableObjectLocationHint | "",
		capacity: serverNumber(env, "CAPACITY"),
		heartbeatIntervalMs: serverNumber(env, "HEARTBEAT_INTERVAL_S", s),
		heartbeatDeadlineMs: milliseconds(env, "HEARTBEAT_DEADLINE_S", 1, s),
		startDeadlineMs: milliseconds(env, "START_DEADLINE_S", 1, s),
		statusPollMs: milliseconds(env, "STATUS_POLL_S", 1, s),
		reservationTtlMs: milliseconds(env, "RESERVATION_TTL_S", 0, s),
		ticketTtlMs: milliseconds(env, "TICKET_TTL_S", 1, s),
		emptyExitS: serverNumber(env, "EMPTY_EXIT_S"),
		serverStatsS: serverNumber(env, "SERVER_STATS_S"),
		serverLogRetentionMs: milliseconds(env, "SERVER_LOG_RETENTION_DAYS", 1, 24 * 60 * 60 * s),
		endedRetentionMs: milliseconds(env, "ENDED_RETENTION_S", 0, s),
		locationsCacheMs: milliseconds(env, "LOCATIONS_CACHE_S", 0, s),
		providerTimeoutMs: milliseconds(env, "PROVIDER_TIMEOUT_S", 0.05, s, 2 ** 31 - 1),
		vcpuPerServer,
	};
	configurations.set(env, cfg);
	return cfg;
}

/** The server's hosted environment has one owner for both launch and the operator's dry-run proof. */
export function serverEnvironment(cfg: Config, serverId: string, origin: string, key: string, secret: string) {
	return [
		{ key: "OVERMATCH_SERVER_ID", value: serverId, hidden: false },
		{ key: "OVERMATCH_API", value: origin, hidden: false },
		{ key: "OVERMATCH_NETCODE_KEY", value: key, hidden: true },
		{ key: "OVERMATCH_REPORT_SECRET", value: secret, hidden: true },
		{ key: "OVERMATCH_CAPACITY", value: String(cfg.capacity), hidden: false },
		{ key: "OVERMATCH_HEARTBEAT_INTERVAL_S", value: String(cfg.heartbeatIntervalMs / 1000), hidden: false },
		{ key: "OVERMATCH_EMPTY_EXIT_S", value: String(cfg.emptyExitS), hidden: false },
		{ key: "OVERMATCH_SERVER_STATS_S", value: String(cfg.serverStatsS), hidden: false },
	];
}
