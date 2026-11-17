import type { Env } from "./config";
import { edgegapFor } from "./edgegap";
import { Ec2 } from "./ec2";
import { bounded, DeadlineError } from "./deadline";

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

/** Capped inventory parses launch time at its edge; unreadable time is local to one host. */
export type CappedDeployment = { id: string; tags: string[]; launchedAt: number | "unreadable" };
export type ListedDeployment = CappedDeployment | { id: string; tags: string[]; launchedAt?: never };

export type Reservation = { server_id: string; created_at: number };

export interface Provider {
	readonly starts: { attempts: number; retryBootInPlace: boolean };
	readonly inventory: { scoped: boolean; maxAgeMs: number | null };
	readonly usdPerVcpuMinute: number | null;
	readonly requiresHttps: boolean;
	verifyRelease?(version: string, timeoutMs: number): Promise<import("./release").ServerRelease>;
	releaseClaim?(serverId: string): Promise<void>;
	reservations?(): Promise<Reservation[]>;
	/** Cutover verification uses the provider's exact launch shape without creating a machine. */
	dryRun?(req: DeployRequest, imageId?: string): Promise<void>;
	/** Resolves to the deployment id. Rejects with `ProviderError`; `definiteRejection` tells a
	 *  refusal (nothing was created) from an unknown outcome (something may have been). */
	deploy(req: DeployRequest): Promise<string>;
	/** The active deployment carrying `tag`, or null. */
	findByTag(tag: string): Promise<string | null>;
	/** The configured inventory (EC2: this Worker environment), all pages up to `maxPages`; `truncated` when more
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

/** DO RPC preserves ordinary exception names, not custom properties. Expected outside failures
 * cross Fleet RPC as data; unexpected exceptions still reject and fail the invocation. */
export type FleetResult<T> = { ok: true; value: T } | { ok: false; cause: "storage" | "provider" };

export async function fleetCall<T>(op: string, timeout: number, call: () => Promise<FleetResult<T>>): Promise<T> {
	const result = await outsideRpc(op, timeout, call);
	if (!result.ok) throw new ProviderError(`${op}:${result.cause}`, 0, "unknown");
	return result.value;
}

/** Retry only platform transport failures, never remote invariant exceptions. */
export async function outsideRpc<T>(op: string, timeout: number, call: () => Promise<T>): Promise<T> {
	try { return await bounded(timeout, call); }
	catch (err) {
		if (err instanceof DeadlineError || (err instanceof Error && (("retryable" in err && err.retryable === true) || ("overloaded" in err && err.overloaded === true)))) {
			throw new ProviderError(`${op}:transport`, 0, "unknown");
		}
		throw err;
	}
}

export function definiteRejection(err: unknown): boolean {
	if (err instanceof ProviderError && err.outcome) return err.outcome !== "unknown";
	return err instanceof ProviderError && err.status >= 400 && err.status < 500 && err.status !== 408;
}

// Removing an adapter removes its accepted configuration name here as well.
const implementations = {
	edgegap: edgegapFor,
	ec2: (env: Env, timeoutMs: number): Provider => new Ec2(env, timeoutMs),
};
type ProviderName = keyof typeof implementations;
const selections = new WeakMap<Env, ProviderName>();
function knownProvider(name: string): name is ProviderName { return Object.hasOwn(implementations, name); }

/** Parse once per immutable Worker environment, at HTTP, scheduled and Durable Object entry. */
export function providerName(env: Env): ProviderName {
	const parsed = selections.get(env);
	if (parsed) return parsed;
	if (!knownProvider(env.PROVIDER)) throw new Error("config: PROVIDER must name an installed provider");
	selections.set(env, env.PROVIDER);
	return env.PROVIDER;
}

export function providerFor(env: Env, timeoutMs: number): Provider {
	return implementations[providerName(env)](env, timeoutMs);
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
