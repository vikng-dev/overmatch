// Every tunable is a wrangler `vars` entry (wrangler.jsonc); this module only parses them. A missing
// or malformed value throws, so a bad deploy fails on its first request instead of running with a
// silent default.

export interface Env {
	REGISTRY: DurableObjectNamespace<import("./registry").Registry>;
	DB: D1Database;
	EDGEGAP_API: string;
	EDGEGAP_APP: string;
	EDGEGAP_PORT_NAME: string;
	EDGEGAP_TOKEN: string;
	EDGEGAP_WEBHOOK_SECRET: string;
	CAPACITY: string;
	HEARTBEAT_INTERVAL_S: string;
	HEARTBEAT_DEADLINE_S: string;
	START_DEADLINE_S: string;
	STATUS_POLL_S: string;
	RESERVATION_TTL_S: string;
	TICKET_TTL_S: string;
	EMPTY_EXIT_S: string;
	ENDED_RETENTION_S: string;
	JOIN_ATTEMPTS: string;
	LOCATIONS_CACHE_S: string;
	VCPU_PER_SERVER: string;
	USD_PER_VCPU_MINUTE: string;
	LOCATION_HINT: string;
	ACCESS_TEAM_DOMAIN: string;
	ACCESS_AUD: string;
}

export interface Config {
	capacity: number;
	heartbeatIntervalMs: number;
	heartbeatDeadlineMs: number;
	startDeadlineMs: number;
	statusPollMs: number;
	reservationTtlMs: number;
	ticketTtlMs: number;
	emptyExitS: number;
	endedRetentionMs: number;
	joinAttempts: number;
	locationsCacheMs: number;
	vcpuPerServer: number;
	usdPerVcpuMinute: number;
}

function num(env: Env, key: keyof Env, min: number): number {
	const raw = env[key];
	const v = typeof raw === "string" && raw.trim() !== "" ? Number(raw) : NaN;
	if (!Number.isFinite(v) || v < min) throw new Error(`config: ${String(key)}=${String(raw)} (need a number >= ${min})`);
	return v;
}

export function config(env: Env): Config {
	const s = 1000;
	return {
		capacity: num(env, "CAPACITY", 1),
		heartbeatIntervalMs: num(env, "HEARTBEAT_INTERVAL_S", 1) * s,
		heartbeatDeadlineMs: num(env, "HEARTBEAT_DEADLINE_S", 1) * s,
		startDeadlineMs: num(env, "START_DEADLINE_S", 1) * s,
		statusPollMs: num(env, "STATUS_POLL_S", 1) * s,
		reservationTtlMs: num(env, "RESERVATION_TTL_S", 0) * s,
		ticketTtlMs: num(env, "TICKET_TTL_S", 1) * s,
		emptyExitS: num(env, "EMPTY_EXIT_S", 0),
		endedRetentionMs: num(env, "ENDED_RETENTION_S", 0) * s,
		joinAttempts: num(env, "JOIN_ATTEMPTS", 1),
		locationsCacheMs: num(env, "LOCATIONS_CACHE_S", 0) * s,
		vcpuPerServer: num(env, "VCPU_PER_SERVER", 0),
		usdPerVcpuMinute: num(env, "USD_PER_VCPU_MINUTE", 0),
	};
}
