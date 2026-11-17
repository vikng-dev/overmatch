import type { Env } from "./config";
import { edgegapFor } from "./edgegap";
import { Ec2 } from "./ec2";

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
	release?: import("./release").ServerRelease;
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
	/** The app version the deployment runs (Edgegap `app_version`), as named at deploy. */
	version: string | null;
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
	status(deploymentId: string, absenceAfter?: number): Promise<DeploymentStatus>;
	/** Idempotent after confirmed termination. EC2 NotFound before absenceAfter remains unknown:
	 *  a newly launched instance can be temporarily invisible in its eventually consistent API. */
	stop(deploymentId: string, absenceAfter?: number): Promise<void>;
	containerLogs?(deploymentId: string): Promise<string>;
	locations(): Promise<LocationData[]>;
	beacons(): Promise<Beacon[]>;
}

/** Safe diagnostics only: operation, status and a locally classified outcome. Provider response
 * messages may echo credentials; never retain them. Edgegap log-body admission uses status 413. */
export class ProviderError extends Error {
	constructor(
		readonly op: string,
		readonly status: number,
		readonly outcome?: "capacity" | "refused" | "unknown" | "releaseMissing" | "releaseUnavailable",
	) {
		super(`provider ${op}: HTTP ${status}`);
		this.name = "ProviderError";
	}
}

export function definiteRejection(err: unknown): boolean {
	if (err instanceof ProviderError && err.outcome) return err.outcome !== "unknown";
	return err instanceof ProviderError && err.status >= 400 && err.status < 500 && err.status !== 408;
}

export function providerFor(env: Env, timeoutMs: number): Provider {
	switch (env.PROVIDER) {
		case "edgegap": return edgegapFor(env, timeoutMs);
		case "ec2": return new Ec2(env, timeoutMs);
		default: throw new Error("config: PROVIDER must name edgegap or ec2");
	}
}

/** Bound outside text before decoding so a stalled or oversized response cannot exhaust the Worker. */
export async function providerText(res: Response, maxBytes: number, op: string, outcome: NonNullable<ProviderError["outcome"]>): Promise<string> {
	if (!res.body) throw new ProviderError(op, res.status, outcome);
	const reader = res.body.getReader();
	const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false });
	let bytes = 0, text = "";
	try {
		while (true) {
			const { done, value } = await reader.read();
			if (done) return text + decoder.decode();
			bytes += value.byteLength;
			if (bytes > maxBytes) {
				await reader.cancel();
				throw new ProviderError(op, res.status, outcome);
			}
			text += decoder.decode(value, { stream: true });
		}
	} finally { reader.releaseLock(); }
}
