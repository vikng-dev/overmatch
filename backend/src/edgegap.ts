// The Edgegap provider adapter. Everything Edgegap-specific — endpoints, auth header, status strings,
// port naming, pagination — stays in this file; the registry sees only `Provider`.
//
// Endpoints (Edgegap OpenAPI as published in docs.edgegap.com, fetched 2026-10-12):
//   POST   /v1/deploy                 marked deprecated upstream, kept because it is the endpoint
//                                     that takes a city `filters` entry (v2 places by IP/coords only)
//   GET    /v1/status/{id}            V1DeploymentStatus
//   DELETE /v1/stop/{id}
//   GET    /v1/deployment/{id}/container-logs stdout/stderr and crash evidence (session.sh cmd_logs)
//   GET    /v1/deployments?query=     active deployments, filtered here by one tag
//   GET    /v1/deployments?page&limit every active deployment, paginated (`pagination.has_next`)
//   GET    /v1/locations?page&limit   paginated (`pagination.has_next`)
//   GET    /v1/locations/beacons      not paginated (`count`)

import { num, type Env } from "./config";
import { bounded, DeadlineError } from "./deadline";

import { cancelProviderBody, providerFetch, providerText, outsideJson, isRecord, isLocation, ProviderError, type Provider, type Beacon, type LocationData, type DeployRequest, type DeploymentStatus, type ListedDeployment } from "./provider";

/** The provider the Worker's env names, every call bounded by `timeoutMs`. */
export function edgegapFor(env: Env, timeoutMs: number): Provider {
	return new Edgegap(env.EDGEGAP_API, env.EDGEGAP_TOKEN, env.EDGEGAP_APP, env.EDGEGAP_PORT_NAME, timeoutMs,
		num(env, "JOIN_ATTEMPTS", 1), num(env, "USD_PER_VCPU_MINUTE", 0));
}

// Resolution: request up to 100 records per exchange to amortize HTTP calls; follow shorter pages.
const PAGE_LIMIT = 100;
// Resolution: refuse a location listing after 50 serial calls rather than follow pagination forever.
const MAX_PAGES = 50;
// Resolution: cap raw log input before decoding/parsing to bound peak memory. DERIVED: 16 MiB
// times roughly four transient body copies budgets about 64 MiB of working data per capture.
export const SERVER_LOG_BODY_MAX_BYTES = 16 * 1024 * 1024;

export class Edgegap implements Provider {
	readonly inventory = { scoped: false, maxAgeMs: null };
	readonly requiresHttps = false;
	readonly starts;
	constructor(
		private readonly api: string,
		private readonly token: string,
		private readonly app: string,
		private readonly portName: string,
		private readonly timeoutMs: number,
		attempts: number,
		readonly usdPerVcpuMinute: number,
	) { this.starts = { attempts, retryBootInPlace: false }; }

	/** One HTTP exchange, request through body read, bounded by `timeoutMs`. A timeout or a network
	 *  failure is a ProviderError with status 0: the provider gave no answer. */
	private async request<T>(op: string, method: string, path: string, body: unknown, handle: (res: Response) => Promise<T>): Promise<T> {
		const encoded = body === undefined ? undefined : JSON.stringify(body);
		const auth = this.token.startsWith("token ") ? this.token : `token ${this.token}`;
		try {
			return await bounded(this.timeoutMs, async (signal) => {
				const res = await providerFetch(op, `${this.api}${path}`, {
					method,
					headers: { authorization: auth, "content-type": "application/json", accept: "application/json" },
					body: encoded,
					signal,
				});
				return handle(res);
			});
		} catch (err) {
			if (err instanceof DeadlineError) throw new ProviderError(op, 0, "unknown");
			throw err;
		}
	}

	private async json<T>(res: Response, what: string, parse: (value: unknown) => T): Promise<T> {
		if (!res.ok) {
			await cancelProviderBody(res, "response");
			throw new ProviderError(what, res.status);
		}
		// Resolution: cap ordinary JSON at 1 MiB before parsing.
		return parse(outsideJson(await providerText(res, 1024 * 1024, what, "unknown"), what));
	}

	async deploy(req: DeployRequest): Promise<string> {
		const body = {
			app_name: this.app,
			version_name: req.version,
			...(req.clientIp ? { ip_list: [req.clientIp] } : {}),
			filters: [{ field: "city", values: [req.city], filter_type: "any" }],
			tags: [req.tag],
			env_vars: req.env.map((e) => ({ key: e.key, value: e.value, is_hidden: e.hidden })),
			webhook_url: req.webhookUrl,
		};
		const out = await this.request("deploy", "POST", "/v1/deploy", body, (res) => this.json(res, "deploy", (value) => {
			if (!isRecord(value) || typeof value.request_id !== "string" || !value.request_id) throw malformed("deploy");
			return value.request_id;
		}));
		return out;
	}

	async status(deploymentId: string): Promise<DeploymentStatus> {
		return this.request("status", "GET", `/v1/status/${encodeURIComponent(deploymentId)}`, undefined, async (res) => {
			if (res.status === 404 || res.status === 410) {
				await cancelProviderBody(res, "response");
				return { phase: "gone", address: null, port: null, tags: [], version: null };
			}
			return interpretStatus(await this.json(res, "status", parseStatus), this.portName);
		});
	}

	async findByTag(tag: string): Promise<string | null> {
		const query = JSON.stringify({ filters: [{ field: "tags", operator: "eq", value: tag }] });
		const out = await this.request("list", "GET", `/v1/deployments?query=${encodeURIComponent(query)}`, undefined, (res) =>
			this.json(res, "list", parseList),
		);
		return out.data.find((d) => d.tags.includes(tag))?.request_id ?? null;
	}

	async deployments(maxPages: number): Promise<{ items: ListedDeployment[]; truncated: boolean }> {
		const items: ListedDeployment[] = [];
		for (let page = 1; page <= maxPages; page++) {
			const out = await this.request("list", "GET", `/v1/deployments?page=${page}&limit=${PAGE_LIMIT}`, undefined, (res) =>
				this.json(res, "list", parseList),
			);
			for (const d of out.data) items.push({ id: d.request_id, tags: d.tags });
			if (!out.pagination.has_next) return { items, truncated: false };
		}
		return { items, truncated: true };
	}

	async stop(deploymentId: string): Promise<void> {
		await this.request("stop", "DELETE", `/v1/stop/${encodeURIComponent(deploymentId)}`, undefined, async (res) => {
			if (res.ok || res.status === 404 || res.status === 410) {
				await cancelProviderBody(res, "response");
				return;
			}
			throw new ProviderError("stop", res.status);
		});
	}

	async containerLogs(deploymentId: string): Promise<string> {
		return this.request("logs", "GET", `/v1/deployment/${encodeURIComponent(deploymentId)}/container-logs`, undefined, async (res) => {
			if (!res.ok) {
				await cancelProviderBody(res, "response");
				throw new ProviderError("logs", res.status);
			}
			const out = outsideJson(await providerText(res, SERVER_LOG_BODY_MAX_BYTES, "logs", "unknown"), "logs");
			if (!out || typeof out !== "object" || Array.isArray(out)) throw new ProviderError("logs", 502);
			const fields = out as Record<string, unknown>;
			for (const key of ["logs", "crash_logs"]) {
				if (fields[key] != null && typeof fields[key] !== "string") throw new ProviderError("logs", 502);
			}
			if (!["logs", "crash_logs", "crash_data"].some((key) => key in fields)) throw new ProviderError("logs", 502);
			return [
				fields.logs ?? "",
				...(fields.crash_logs != null ? ["--- crash_logs ---", fields.crash_logs] : []),
				...(fields.crash_data != null ? ["--- crash_data ---", JSON.stringify(fields.crash_data)] : []),
			].join("\n");
		});
	}

	async locations(): Promise<LocationData[]> {
		const all: LocationData[] = [];
		for (let page = 1; page <= MAX_PAGES; page++) {
			const out = await this.request("locations", "GET", `/v1/locations?page=${page}&limit=${PAGE_LIMIT}`, undefined, (res) =>
				this.json(res, "locations", parseLocations),
			);
			all.push(...out.locations);
			if (!out.pagination.has_next) return all;
		}
		throw new ProviderError("locations", 502);
	}

	async beacons(): Promise<Beacon[]> {
		const out = await this.request("beacons", "GET", "/v1/locations/beacons", undefined, (res) =>
			this.json(res, "beacons", parseBeacons),
		);
		return out;
	}
}

interface V1DeploymentStatus {
	current_status?: string;
	running?: boolean;
	error?: boolean;
	public_ip?: string;
	ports?: Record<string, { external?: number } | undefined>;
	tags?: string[];
	app_version?: string;
}

/** Status strings are "Status.READY", "Status.ERROR", "Status.TERMINATED", … (MEASURED in the risk
 *  test); the booleans are read first because they are the documented fields. */
export function interpretStatus(s: V1DeploymentStatus, portName: string): DeploymentStatus {
	const current = (s.current_status ?? "").toUpperCase();
	const base = { tags: s.tags ?? [], version: s.app_version ?? null };
	if (s.error || current.includes("ERROR")) return { phase: "error", address: null, port: null, ...base };
	if (current.includes("TERMINAT") || current.includes("STOP")) return { phase: "terminated", address: null, port: null, ...base };
	const port = s.ports?.[portName]?.external ?? null;
	if (s.running && s.public_ip && port !== null) return { phase: "ready", address: s.public_ip, port, ...base };
	return { phase: "pending", address: null, port: null, ...base };
}

const malformed = (op: string) => new ProviderError(op, 502, "unknown");
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every((item) => typeof item === "string");
function pagination(value: Record<string, unknown>, op: string): { has_next: boolean } {
	if (!isRecord(value.pagination) || typeof value.pagination.has_next !== "boolean") throw malformed(op);
	return { has_next: value.pagination.has_next };
}
function parseList(value: unknown): { data: { request_id: string; tags: string[] }[]; pagination: { has_next: boolean } } {
	if (!isRecord(value) || !Array.isArray(value.data)) throw malformed("list");
	const data = value.data.map((row) => {
		if (!isRecord(row) || typeof row.request_id !== "string" || !row.request_id || (row.tags !== undefined && !strings(row.tags))) throw malformed("list");
		return { request_id: row.request_id, tags: row.tags === undefined ? [] : row.tags as string[] };
	});
	return { data, pagination: pagination(value, "list") };
}
function parseStatus(value: unknown): V1DeploymentStatus {
	if (!isRecord(value) || !["current_status", "running", "error"].some((key) => key in value)) throw malformed("status");
	for (const key of ["current_status", "public_ip", "app_version"]) if (value[key] !== undefined && typeof value[key] !== "string") throw malformed("status");
	for (const key of ["running", "error"]) if (value[key] !== undefined && typeof value[key] !== "boolean") throw malformed("status");
	if (value.tags !== undefined && !strings(value.tags)) throw malformed("status");
	if (value.ports !== undefined) {
		if (!isRecord(value.ports)) throw malformed("status");
		for (const port of Object.values(value.ports)) {
			if (!isRecord(port) || (port.external !== undefined && (!Number.isInteger(port.external) || Number(port.external) < 1 || Number(port.external) > 65535))) throw malformed("status");
		}
	}
	return value as V1DeploymentStatus;
}
function parseLocations(value: unknown): { locations: LocationData[]; pagination: { has_next: boolean } } {
	if (!isRecord(value) || !Array.isArray(value.locations) || !value.locations.every(isLocation)) throw malformed("locations");
	return { locations: value.locations, pagination: pagination(value, "locations") };
}
function parseBeacons(value: unknown): Beacon[] {
	if (!isRecord(value) || !Array.isArray(value.locations)) throw malformed("beacons");
	return value.locations.map((row) => {
		if (!isRecord(row) || typeof row.host !== "string" || !row.host || !isLocation(row.location)
			|| (row.fqdn != null && typeof row.fqdn !== "string")) throw malformed("beacons");
		for (const key of ["udp_port", "tcp_port"]) if (row[key] != null && (!Number.isInteger(row[key]) || Number(row[key]) < 1 || Number(row[key]) > 65535)) throw malformed("beacons");
		return { host: row.host, fqdn: row.fqdn == null ? null : row.fqdn as string,
			udp_port: row.udp_port == null ? null : row.udp_port as number, tcp_port: row.tcp_port == null ? null : row.tcp_port as number, location: row.location };
	});
}
