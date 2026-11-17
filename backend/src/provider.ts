import { config, ConfigurationError, type Env } from "./config";
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
	/** Resolves to the deployment id; outside failures reject with `ProviderError`, bugs propagate.
	 *  `definiteRejection` distinguishes refusal from unknown outcome (something may exist). */
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

/** Cloudflare names reset and overload faults with these flags; SQL/schema errors lack them.
 * https://developers.cloudflare.com/durable-objects/best-practices/error-handling/ */
export function isPlatformFault(err: unknown): boolean {
	return err instanceof Error && (("retryable" in err && err.retryable === true) || ("overloaded" in err && err.overloaded === true));
}

/** Serialize and validate caller-owned arguments before entering the network catch. */
export async function providerFetch(op: string, input: string | URL, init?: RequestInit): Promise<Response> {
	new Request(input, init);
	try { return await fetch(input, init); }
	catch (err) {
		// Fetch reports network failure as TypeError, with no narrower subtype after argument validation.
		if (err instanceof TypeError || (err instanceof DOMException && (err.name === "AbortError" || err.name === "TimeoutError"))) throw new ProviderError(op, 0, "unknown");
		throw err;
	}
}

export function outsideJson(raw: string, op: string): unknown {
	try { return JSON.parse(raw); }
	catch (err) {
		if (err instanceof SyntaxError) throw new ProviderError(op, 0, "unknown");
		throw err;
	}
}

export function isRecord(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}
export function isLocation(value: unknown): value is LocationData {
	return isRecord(value) && ["city", "country", "continent"].every((key) => typeof value[key] === "string")
		&& typeof value.latitude === "number" && Number.isFinite(value.latitude) && typeof value.longitude === "number" && Number.isFinite(value.longitude);
}
export function isBeacon(value: unknown): value is Beacon {
	return isRecord(value) && typeof value.host === "string" && (value.fqdn === null || typeof value.fqdn === "string")
		&& ["udp_port", "tcp_port"].every((key) => value[key] === null || (Number.isInteger(value[key]) && Number(value[key]) > 0 && Number(value[key]) <= 65535))
		&& isLocation(value.location);
}

/** Classify a foreground RPC without changing the operation's existing lifetime. */
export async function platformCall<T>(op: string, call: () => Promise<T>): Promise<T> {
	try { return await call(); }
	catch (err) {
		if (isPlatformFault(err)) throw new ProviderError(`${op}:transport`, 0, "unknown");
		throw err;
	}
}

/** Classify platform transport failures, never remote invariant exceptions. */
export async function outsideRpc<T>(op: string, timeout: number, call: () => Promise<T>): Promise<T> {
	try { return await bounded(timeout, call); }
	catch (err) {
		if (err instanceof DeadlineError || isPlatformFault(err)) {
			throw new ProviderError(`${op}:transport`, 0, "unknown");
		}
		throw err;
	}
}

// D1 exposes these infrastructure failures as messages, not typed codes. Match only documented
// causes behind D1_ERROR; SQL/schema errors must propagate. https://developers.cloudflare.com/d1/observability/debug-d1/
const D1_OUTSIDE_FAILURES = new Set([
	"D1 DB reset because its code was updated.",
	"Internal error while starting up D1 DB storage caused object to be reset.",
	"Network connection lost.",
	"Replica disconnected from primary.",
	"Internal error in D1 DB storage caused object to be reset.",
	"Cannot resolve D1 DB due to transient issue on remote node.",
	"Can't read from request stream because client disconnected.",
	"D1 DB is overloaded. Requests queued for too long.",
	"D1 DB is overloaded. Too many requests queued.",
	"D1 DB is overloaded. Too many resources consumed within a 10 second window",
]);
export async function d1Call<T>(op: string, timeout: number, call: () => Promise<T>): Promise<T> {
	try { return await outsideRpc(op, timeout, call); }
	catch (err) {
		if (err instanceof Error && err.message.startsWith("D1_ERROR: ") && D1_OUTSIDE_FAILURES.has(err.message.slice("D1_ERROR: ".length))) {
			throw new ProviderError(`${op}:storage`, 0, "unknown");
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
export function parseProviderName(name: string): ProviderName {
	if (!Object.hasOwn(implementations, name)) throw new ConfigurationError("PROVIDER", "must name an installed provider");
	return name as ProviderName;
}

export function providerFor(env: Env, timeoutMs: number): Provider {
	return implementations[config(env).provider.name](env, timeoutMs);
}

/** Bound outside text before decoding so a stalled or oversized response cannot exhaust the Worker. */
export async function providerText(res: Response, maxBytes: number, op: string, outcome: NonNullable<ProviderError["outcome"]>): Promise<string> {
	if (!res.body) throw new ProviderError(op, res.status, outcome);
	const reader = res.body.getReader();
	const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false });
	let bytes = 0, text = "";
	const decode = (value?: Uint8Array) => {
		try { return decoder.decode(value, { stream: value !== undefined }); }
		catch (err) {
			if (err instanceof TypeError) throw new ProviderError(op, res.status, outcome);
			throw err;
		}
	};
	try {
		while (true) {
			const { done, value } = await providerIo(() => reader.read(), op, res.status, outcome);
			if (done) return text + decode();
			bytes += value.byteLength;
			if (bytes > maxBytes) {
				await providerIo(() => reader.cancel(), op, res.status, outcome);
				throw new ProviderError(op, 413, outcome);
			}
			text += decode(value);
		}
	} finally { reader.releaseLock(); }
}

/** Only native network stream operations belong here; application callbacks must run outside it. */
async function providerIo<T>(call: () => Promise<T>, op: string, status: number, outcome: NonNullable<ProviderError["outcome"]>): Promise<T> {
	try { return await call(); }
	catch {
		// Network response streams reject with arbitrary platform errors, without a stable subtype.
		throw new ProviderError(op, status, outcome);
	}
}
export async function cancelProviderBody(res: Response, op: string): Promise<void> {
	if (!res.body) return;
	if (res.body.locked) throw new Error("provider body must be unlocked before cancellation");
	await providerIo(() => res.body!.cancel(), op, res.status, "unknown");
}
