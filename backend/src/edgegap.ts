// The one provider adapter. Everything Edgegap-specific — endpoints, auth header, status strings,
// port naming, pagination — stays in this file; the registry sees only `Provider`.
//
// Endpoints (Edgegap OpenAPI as published in docs.edgegap.com, fetched 2026-10-12):
//   POST   /v1/deploy                 marked deprecated upstream, kept because it is the endpoint
//                                     that takes a city `filters` entry (v2 places by IP/coords only)
//   GET    /v1/status/{id}            V1DeploymentStatus
//   DELETE /v1/stop/{id}
//   GET    /v1/deployments?query=     active deployments, filtered here by one tag
//   GET    /v1/deployments?page&limit every active deployment, paginated (`pagination.has_next`)
//   GET    /v1/locations?page&limit   paginated (`pagination.has_next`)
//   GET    /v1/locations/beacons      not paginated (`count`)

import type { Env } from "./config";
import { bounded } from "./deadline";

export interface LocationData {
	city: string;
	country: string;
	continent: string;
	latitude: number;
	longitude: number;
}

export interface Beacon {
	host: string;
	fqdn: string | null;
	udp_port: number | null;
	tcp_port: number | null;
	location: LocationData;
}

export interface DeployRequest {
	version: string;
	city: string;
	/** Unique to the claim; how a deployment whose deploy response was lost is found again. */
	tag: string;
	clientIp: string | null;
	env: { key: string; value: string; hidden: boolean }[];
	webhookUrl: string;
}

export type DeploymentPhase = "pending" | "ready" | "error" | "terminated" | "gone";

export interface DeploymentStatus {
	phase: DeploymentPhase;
	address: string | null;
	port: number | null;
	tags: string[];
}

export interface ListedDeployment {
	id: string;
	tags: string[];
}

export interface Provider {
	/** Resolves to the deployment id. Rejects with `ProviderError`; `definiteRejection` tells a
	 *  refusal (nothing was created) from an unknown outcome (something may have been). */
	deploy(req: DeployRequest): Promise<string>;
	/** The active deployment carrying `tag`, or null. */
	findByTag(tag: string): Promise<string | null>;
	/** Every active deployment of the account, all pages up to `maxPages`; `truncated` when more
	 *  pages remained. */
	deployments(maxPages: number): Promise<{ items: ListedDeployment[]; truncated: boolean }>;
	status(deploymentId: string): Promise<DeploymentStatus>;
	/** Idempotent: an already-stopped or unknown deployment is success. */
	stop(deploymentId: string): Promise<void>;
	locations(): Promise<LocationData[]>;
	beacons(): Promise<Beacon[]>;
}

/** A failed provider call. It carries the operation and HTTP status only; the response body is
 *  discarded unread, because it may echo request values (see reasons.ts). */
export class ProviderError extends Error {
	constructor(
		readonly op: string,
		readonly status: number,
	) {
		super(`edgegap ${op}: HTTP ${status}`);
		this.name = "ProviderError";
	}
}

/** The provider the Worker's env names, every call bounded by `timeoutMs`. */
export function edgegapFor(env: Env, timeoutMs: number): Provider {
	return new Edgegap(env.EDGEGAP_API, env.EDGEGAP_TOKEN, env.EDGEGAP_APP, env.EDGEGAP_PORT_NAME, timeoutMs);
}

const PAGE_LIMIT = 100;
const MAX_PAGES = 50;

export class Edgegap implements Provider {
	constructor(
		private readonly api: string,
		private readonly token: string,
		private readonly app: string,
		private readonly portName: string,
		private readonly timeoutMs: number,
	) {}

	/** One HTTP exchange, request through body read, bounded by `timeoutMs`. A timeout or a network
	 *  failure is a ProviderError with status 0: the provider gave no answer. */
	private async request<T>(op: string, method: string, path: string, body: unknown, handle: (res: Response) => Promise<T>): Promise<T> {
		const auth = this.token.startsWith("token ") ? this.token : `token ${this.token}`;
		try {
			return await bounded(this.timeoutMs, async (signal) => {
				const res = await fetch(`${this.api}${path}`, {
					method,
					headers: { authorization: auth, "content-type": "application/json", accept: "application/json" },
					body: body === undefined ? undefined : JSON.stringify(body),
					signal,
				});
				return handle(res);
			});
		} catch (err) {
			if (err instanceof ProviderError) throw err;
			throw new ProviderError(op, 0);
		}
	}

	private async json<T>(res: Response, what: string): Promise<T> {
		if (!res.ok) {
			await res.body?.cancel();
			throw new ProviderError(what, res.status);
		}
		return (await res.json()) as T;
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
		const out = await this.request("deploy", "POST", "/v1/deploy", body, (res) => this.json<{ request_id: string }>(res, "deploy"));
		if (!out.request_id) throw new ProviderError("deploy", 502);
		return out.request_id;
	}

	async status(deploymentId: string): Promise<DeploymentStatus> {
		return this.request("status", "GET", `/v1/status/${encodeURIComponent(deploymentId)}`, undefined, async (res) => {
			if (res.status === 404 || res.status === 410) {
				await res.body?.cancel();
				return { phase: "gone", address: null, port: null, tags: [] };
			}
			return interpretStatus(await this.json<V1DeploymentStatus>(res, "status"), this.portName);
		});
	}

	async findByTag(tag: string): Promise<string | null> {
		const query = JSON.stringify({ filters: [{ field: "tags", operator: "eq", value: tag }] });
		const out = await this.request("list", "GET", `/v1/deployments?query=${encodeURIComponent(query)}`, undefined, (res) =>
			this.json<{ data?: { request_id?: string; tags?: string[] }[] }>(res, "list"),
		);
		return (out.data ?? []).find((d) => d.request_id && (d.tags ?? []).includes(tag))?.request_id ?? null;
	}

	async deployments(maxPages: number): Promise<{ items: ListedDeployment[]; truncated: boolean }> {
		const items: ListedDeployment[] = [];
		for (let page = 1; page <= maxPages; page++) {
			const out = await this.request("list", "GET", `/v1/deployments?page=${page}&limit=${PAGE_LIMIT}`, undefined, (res) =>
				this.json<{ data?: { request_id?: string; tags?: string[] }[]; pagination?: { has_next?: boolean } }>(res, "list"),
			);
			for (const d of out.data ?? []) if (d.request_id) items.push({ id: d.request_id, tags: d.tags ?? [] });
			if (!out.pagination?.has_next) return { items, truncated: false };
		}
		return { items, truncated: true };
	}

	async stop(deploymentId: string): Promise<void> {
		await this.request("stop", "DELETE", `/v1/stop/${encodeURIComponent(deploymentId)}`, undefined, async (res) => {
			if (res.ok || res.status === 404 || res.status === 410) {
				await res.body?.cancel();
				return;
			}
			await this.json(res, "stop");
		});
	}

	async locations(): Promise<LocationData[]> {
		const all: LocationData[] = [];
		for (let page = 1; page <= MAX_PAGES; page++) {
			const out = await this.request("locations", "GET", `/v1/locations?page=${page}&limit=${PAGE_LIMIT}`, undefined, (res) =>
				this.json<{ locations?: LocationData[]; pagination?: { has_next?: boolean } }>(res, "locations"),
			);
			all.push(...(out.locations ?? []));
			if (!out.pagination?.has_next) return all;
		}
		throw new ProviderError("locations", 502);
	}

	async beacons(): Promise<Beacon[]> {
		const out = await this.request("beacons", "GET", "/v1/locations/beacons", undefined, (res) =>
			this.json<{ locations?: Partial<Beacon>[] }>(res, "beacons"),
		);
		return (out.locations ?? [])
			.filter((b): b is Partial<Beacon> & { host: string; location: LocationData } => !!b.host && !!b.location)
			.map((b) => ({
				host: b.host,
				fqdn: b.fqdn ?? null,
				udp_port: b.udp_port ?? null,
				tcp_port: b.tcp_port ?? null,
				location: b.location,
			}));
	}
}

interface V1DeploymentStatus {
	current_status?: string;
	running?: boolean;
	error?: boolean;
	public_ip?: string;
	ports?: Record<string, { external?: number } | undefined>;
	tags?: string[];
}

/** Status strings are "Status.READY", "Status.ERROR", "Status.TERMINATED", … (MEASURED in the risk
 *  test); the booleans are read first because they are the documented fields. */
export function interpretStatus(s: V1DeploymentStatus, portName: string): DeploymentStatus {
	const current = (s.current_status ?? "").toUpperCase();
	const tags = s.tags ?? [];
	if (s.error || current.includes("ERROR")) return { phase: "error", address: null, port: null, tags };
	if (current.includes("TERMINAT") || current.includes("STOP")) return { phase: "terminated", address: null, port: null, tags };
	const port = s.ports?.[portName]?.external ?? null;
	if (s.running && s.public_ip && port !== null) return { phase: "ready", address: s.public_ip, port, tags };
	return { phase: "pending", address: null, port: null, tags };
}

/** True when Edgegap answered the deploy with a refusal, so nothing was created. A timeout, a lost
 *  connection, a 5xx or a 408 leaves the outcome unknown: a deployment may exist. */
export function definiteRejection(err: unknown): boolean {
	return err instanceof ProviderError && err.status >= 400 && err.status < 500 && err.status !== 408;
}
