// EC2 Query API 2016-11-15. Only RunInstances, DescribeInstances and TerminateInstances;
// native resolve:ssm: uses the policy's one public parameter without another Worker AWS action.
import hosting from "../aws-hosting.json";
import production from "../production-server.json";
import type { Env } from "./config";
import { bounded, DeadlineError } from "./deadline";
import { parseXml, type Xml } from "./ec2-xml";
import { host, machineScript } from "./host.generated";
import { fleetCall, providerText, ProviderError, type Provider, type DeployRequest, type DeploymentStatus, type CappedDeployment } from "./provider";
import { queryString, signV4 } from "./sigv4";
import { serverRelease } from "./release";

// Owner choices: hosting design decisions 3 and 5, and section 5's independent lifetime guard.
export const MAX_HOSTS = hosting.maxServers;
export const BOOT_ATTEMPTS = hosting.startAttempts;
export const HOST_CAP_MIN = hosting.lifetimeMinutes;
export const EC2_LOCATION = hosting.location;
// Resolution: DescribeInstances pages are limited to 100 records; a 1 MiB body cap bounds parsing memory.
const API = `https://ec2.${host.region}.amazonaws.com/`;
// Resolution: one bounded retransmission recovers a lost answer without holding a join indefinitely.
const CALL_ATTEMPTS = 2;
const CAPACITY = new Set(["InsufficientInstanceCapacity", "InsufficientHostCapacity", "InstanceLimitExceeded", "VcpuLimitExceeded", "InsufficientFreeAddressesInSubnet"]);

const REFUSED = new Set(["UnauthorizedOperation", "AuthFailure", "InvalidClientTokenId", "SignatureDoesNotMatch", "RequestExpired", "MissingAuthenticationToken", "InvalidParameter", "InvalidParameterValue", "InvalidParameterCombination", "MissingParameter", "Unsupported", "UnsupportedOperation", "InvalidAMIID.NotFound", "InvalidAMIID.Unavailable", "InvalidAMIID.Malformed", "InvalidBlockDeviceMapping", "InvalidSubnetID.NotFound", "InvalidSubnetID.Malformed", "InvalidGroup.NotFound", "InvalidGroupId.Malformed", "InvalidKeyPair.NotFound", "InvalidInstanceID.Malformed", "OperationNotPermitted", "Blocked", "PendingVerification"]);

export function ec2Outcome(code: string): "capacity" | "refused" | "unknown" {
	if (CAPACITY.has(code)) return "capacity";
	if (REFUSED.has(code)) return "refused";
	// Unknown codes cannot prove that nothing was created. Preserve the claim for reconciliation.
	return "unknown";
}

function shellQuote(value: string): string { return `'${value.replaceAll("'", `'"'"'`)}'`; }
export function hostedScript(req: DeployRequest): string {
	if (!req.release) throw new Error("EC2 launch must have a verified release from the Registry");
	const values = Object.fromEntries(req.env.map((e) => [e.key, e.value]));
	for (const name of ["OVERMATCH_API", "OVERMATCH_SERVER_ID", "OVERMATCH_REPORT_SECRET"]) {
		if (!values[name]) throw new Error(`EC2 launch requires ${name}`);
	}
	if (new URL(values.OVERMATCH_API).protocol !== "https:") throw new ProviderError("insecureOrigin", 0, "refused");
	const environment = req.env.map(({ key, value }) => {
		if (!/^OVERMATCH_[A-Z_]+$/.test(key) || /[\r\n\0]/.test(value)) throw new Error("invalid hosted environment");
		return `${key}="${value.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"\n`;
	}).join("");
	const settings = {
		ROLE: "hosted", CAP_MIN: String(HOST_CAP_MIN), ARCHIVE_URL: req.release.url, ARCHIVE_SHA256: req.release.checksum,
		HOSTED_ENV_B64: btoa(environment), HOSTED_API: values.OVERMATCH_API,
		HOSTED_SERVER_ID: values.OVERMATCH_SERVER_ID, HOSTED_REPORT_SECRET: values.OVERMATCH_REPORT_SECRET,
	};
	const script = machineScript.replace("# @settings\n", Object.entries(settings).map(([k, v]) => `${k}=${shellQuote(v)}\n`).join(""));
	// EC2's documented unencoded user-data limit. Refuse before a launch can carry incomplete boot code.
	if (new TextEncoder().encode(script).byteLength > 16 * 1024) throw new ProviderError("userdata", 0, "refused");
	return script;
}

type Network = { securityGroupId: string; subnetIds: string[] };
function network(raw: string | undefined): Network {
	try {
		const n: unknown = JSON.parse(raw ?? "");
		if (n && typeof n === "object" && "securityGroupId" in n && "subnetIds" in n
			&& Object.keys(n).length === 2 && typeof n.securityGroupId === "string" && /^sg-[0-9a-f]{17}$/.test(n.securityGroupId)
			&& Array.isArray(n.subnetIds) && n.subnetIds.length > 0
			&& n.subnetIds.every((s: unknown) => typeof s === "string" && /^subnet-[0-9a-f]{17}$/.test(s))
			&& new Set(n.subnetIds).size === n.subnetIds.length) return n as Network;
	} catch { /* The operator sees a typed refusal, never the secret's contents. */ }
	throw new ProviderError("config", 0, "refused");
}

function launchFields(req: DeployRequest, n: Network, subnet: string, script: string, workerEnv: string): Record<string, string> {
	const params: Record<string, string> = {
		ImageId: `resolve:ssm:${host.parameter}`, InstanceType: production.instance_type, MinCount: "1", MaxCount: "1",
		ClientToken: req.tag, "Placement.Tenancy": "default", "InstanceMarketOptions.MarketType": "on-demand",
		"MetadataOptions.HttpTokens": "required", "MetadataOptions.HttpEndpoint": "enabled", "MetadataOptions.HttpPutResponseHopLimit": "1",
		InstanceInitiatedShutdownBehavior: "terminate", DisableApiTermination: "false", UserData: btoa(String.fromCharCode(...new TextEncoder().encode(script))),
		"NetworkInterface.1.DeviceIndex": "0", "NetworkInterface.1.SubnetId": subnet,
		"NetworkInterface.1.SecurityGroupId.1": n.securityGroupId,
		"NetworkInterface.1.AssociatePublicIpAddress": "true", "NetworkInterface.1.DeleteOnTermination": "true",
		"BlockDeviceMapping.1.DeviceName": host.rootDeviceName, "BlockDeviceMapping.1.Ebs.VolumeType": "gp3",
		"BlockDeviceMapping.1.Ebs.VolumeSize": String(host.rootSizeGiB), "BlockDeviceMapping.1.Ebs.Iops": String(host.iops),
		"BlockDeviceMapping.1.Ebs.Throughput": String(host.throughput), "BlockDeviceMapping.1.Ebs.DeleteOnTermination": "true",
	};
	["instance", "volume", "network-interface"].forEach((kind, i) => {
		const prefix = `TagSpecification.${i + 1}`;
		params[`${prefix}.ResourceType`] = kind;
		Object.entries({ ...host.tags, WorkerEnvironment: workerEnv, Claim: req.tag, Version: req.version }).forEach(([key, value], j) => {
			params[`${prefix}.Tag.${j + 1}.Key`] = key;
			params[`${prefix}.Tag.${j + 1}.Value`] = value;
		});
	});
	return params;
}

export class Ec2 implements Provider {
	readonly starts = { attempts: BOOT_ATTEMPTS, retryBootInPlace: true };
	readonly inventory = { scoped: true, maxAgeMs: (HOST_CAP_MIN + hosting.sweepMarginMinutes) * 60_000 };
	readonly usdPerVcpuMinute = null;
	readonly requiresHttps = true;
	readonly verifyRelease = serverRelease;
	private readonly workerEnv: string;
	constructor(private readonly env: Env, private readonly timeoutMs: number) {
		if (typeof env.WORKER_ENV !== "string" || !/^[a-z][a-z0-9-]{0,62}$/.test(env.WORKER_ENV)) throw new Error("config: WORKER_ENV must name this deployment (lowercase letters, digits, hyphens)");
		this.workerEnv = env.WORKER_ENV;
	}
	releaseClaim(serverId: string): Promise<void> { return fleetCall("fleetRelease", this.timeoutMs, () => this.env.FLEET.getByName("ec2").release(serverId)); }
	reservations() { return fleetCall("fleetList", this.timeoutMs, () => this.env.FLEET.getByName("ec2").reservations()); }

	private async query(action: string, params: Record<string, string>, outerSignal?: AbortSignal): Promise<Xml> {
		if (!this.env.AWS_ACCESS_KEY_ID || !this.env.AWS_SECRET_ACCESS_KEY) throw new ProviderError("credentials", 0, "refused");
		const body = queryString({ Action: action, Version: "2016-11-15", ...params });
		try {
			return await bounded(this.timeoutMs, async (signal) => {
				const headers = await signV4({ method: "POST", url: new URL(API), body,
					headers: { "content-type": "application/x-www-form-urlencoded; charset=utf-8" },
					accessKey: this.env.AWS_ACCESS_KEY_ID!, secretKey: this.env.AWS_SECRET_ACCESS_KEY!,
					region: host.region, service: "ec2", date: new Date() });
				const res = await fetch(API, { method: "POST", headers, body, signal: outerSignal ? AbortSignal.any([signal, outerSignal]) : signal });
				const xml = parseXml(await providerText(res, 1024 * 1024, action, "unknown"));
				const errors = xml.all("Errors");
				if (errors.length) {
					const code = errors[0].one("Error").one("Code").text;
					if (code === "DryRunOperation" && action === "RunInstances" && params.DryRun === "true") throw new DryRunAccepted();
					if (code === "InvalidInstanceID.NotFound") throw new MissingInstance();
					// Code, not HTTP status: capacity is commonly a 5xx. No response message is retained.
					throw new ProviderError(action, res.status, ec2Outcome(code));
				}
				if (!res.ok || xml.name !== `${action}Response`) throw new ProviderError(action, res.status, "unknown");
				return xml;
			});
		} catch (err) {
			if (err instanceof ProviderError || err instanceof MissingInstance || err instanceof DryRunAccepted) throw err;
			throw new ProviderError(action, 0, "unknown");
		}
	}

	async deploy(req: DeployRequest): Promise<string> {
		try { return await bounded(this.timeoutMs, (signal) => this.launch(req, signal)); }
		catch (err) {
			if (err instanceof DeadlineError) throw new ProviderError("RunInstances", 0, "unknown");
			throw err;
		}
	}

	private async launch(req: DeployRequest, signal: AbortSignal): Promise<string> {
		if (!this.env.AWS_ACCESS_KEY_ID || !this.env.AWS_SECRET_ACCESS_KEY) throw new ProviderError("credentials", 0, "refused");
		const n = network(this.env.AWS_HOST_CONFIG);
		const script = hostedScript(req);
		const serverId = req.env.find((e) => e.key === "OVERMATCH_SERVER_ID")!.value;
		const admitted = await fleetCall("fleetReserve", this.timeoutMs, () => this.env.FLEET.getByName("ec2").reserve(serverId));
		if (!admitted) throw new ProviderError("cap", 0, "capacity");
		for (const subnet of n.subnetIds) {
			const params = launchFields(req, n, subnet, script, this.workerEnv);
			for (let attempt = 0; attempt < CALL_ATTEMPTS; attempt++) {
				try {
					if (signal.aborted) throw new ProviderError("RunInstances", 0, "unknown");
					const xml = await this.query("RunInstances", params, signal);
					const id = xml.one("instancesSet").one("item").one("instanceId").text;
					if (!/^i-[0-9a-f]{17}$/.test(id)) throw new ProviderError("RunInstances", 0, "unknown");
					return id;
				} catch (err) {
					if (!(err instanceof ProviderError)) throw new ProviderError("RunInstances", 0, "unknown");
					if (err.outcome === "capacity") break; // Definite refusal: the next subnet may have room.
					if (err.outcome === "refused" || attempt + 1 === CALL_ATTEMPTS) throw err;
				}
			}
		}
		throw new ProviderError("RunInstances", 0, "capacity");
	}

	/** Operator proof of the exact launch shape, using the key held only by the proving Worker. */
	async dryRun(req: DeployRequest, imageId?: string): Promise<void> {
		const n = network(this.env.AWS_HOST_CONFIG);
		for (const subnet of n.subnetIds) {
			const params = launchFields(req, n, subnet, hostedScript(req), this.workerEnv);
			params.DryRun = "true";
			if (imageId !== undefined) params.ImageId = imageId;
			try {
				await this.query("RunInstances", params);
				throw new ProviderError("dryRunUnconfirmed", 0, "unknown");
			} catch (err) {
				if (!(err instanceof DryRunAccepted)) throw err;
			}
		}
	}

	private async describe(params: Record<string, string>): Promise<{ items: Xml[]; next: string | null }> {
		const xml = await this.query("DescribeInstances", params);
		return { items: xml.one("reservationSet").all("item").flatMap((r) => r.one("instancesSet").all("item")), next: xml.optional("nextToken") };
	}

	async deployments(maxPages: number): Promise<{ items: CappedDeployment[]; truncated: boolean }> {
		const items: CappedDeployment[] = [];
		let next: string | null = null;
		for (let page = 0; page < maxPages; page++) {
			const params: Record<string, string> = { MaxResults: "100" };
			const tags = Object.entries({ ...host.tags, WorkerEnvironment: this.workerEnv });
			tags.forEach(([k, v], i) => { params[`Filter.${i + 1}.Name`] = `tag:${k}`; params[`Filter.${i + 1}.Value.1`] = v; });
			const state = `Filter.${tags.length + 1}`;
			params[`${state}.Name`] = "instance-state-name";
			["pending", "running", "shutting-down", "stopping", "stopped"].forEach((v, i) => { params[`${state}.Value.${i + 1}`] = v; });
			if (next) params.NextToken = next;
			const out = await this.describe(params);
			for (const x of out.items) {
				const launchedAt = Date.parse(x.optional("launchTime") ?? "");
				items.push({ id: x.one("instanceId").text, tags: claimTags(x), launchedAt: Number.isFinite(launchedAt) ? launchedAt : "unreadable" });
			}
			next = out.next;
			if (!next) return { items, truncated: false };
		}
		return { items, truncated: true };
	}

	async findByTag(tag: string): Promise<string | null> {
		const out = await this.deployments(50);
		if (out.truncated) throw new ProviderError("list", 0, "unknown");
		const found = out.items.filter((x) => x.tags.includes(tag));
		if (found.length > 1) throw new ProviderError("ambiguousClaim", 0, "unknown");
		return found[0]?.id ?? null; // No visible instance yet: the registry retains the claim until its deadline.
	}

	async status(id: string, absenceAfter?: number): Promise<DeploymentStatus> {
		try {
			const out = await this.describe({ "InstanceId.1": id });
			if (out.items.length !== 1) throw new ProviderError("status", 0, "unknown");
			const x = out.items[0];
			if (x.one("instanceId").text !== id) throw new ProviderError("status", 0, "unknown");
			const state = x.one("instanceState").one("name").text;
			const address = x.optional("ipAddress");
			const tags = x.one("tagSet").all("item");
			const version = tags.find((t) => t.one("key").text === "Version")?.one("value").text ?? null;
			let phase: DeploymentStatus["phase"];
			switch (state) {
				case "pending": phase = "pending"; break;
				case "running": phase = address ? "ready" : "pending"; break;
				case "shutting-down": case "stopping": phase = "pending"; break;
				case "stopped": phase = "error"; break;
				case "terminated": phase = "terminated"; break;
				default: throw new ProviderError("state", 0, "unknown");
			}
			return { phase, address, port: address ? host.port : null, tags: claimTags(x), version };
		} catch (err) {
			if (err instanceof MissingInstance) {
				if (absenceAfter !== undefined && Date.now() >= absenceAfter) return { phase: "gone", address: null, port: null, tags: [], version: null };
				throw new ProviderError("instanceNotVisible", 0, "unknown");
			}
			throw err;
		}
	}

	async stop(id: string, absenceAfter?: number): Promise<void> {
		try {
			const xml = await this.query("TerminateInstances", { "InstanceId.1": id });
			const item = xml.one("instancesSet").one("item");
			if (item.one("instanceId").text !== id) throw new ProviderError("stop", 0, "unknown");
			if (item.one("currentState").one("name").text === "terminated") return;
			// Terminate accepted is not terminate confirmed: shutting-down retains the slot and retries.
			if (!["terminated", "gone"].includes((await this.status(id, absenceAfter)).phase)) throw new ProviderError("terminating", 0, "unknown");
		} catch (err) {
			if (err instanceof MissingInstance) {
				if (absenceAfter !== undefined && Date.now() >= absenceAfter) return;
				throw new ProviderError("instanceNotVisible", 0, "unknown");
			}
			throw err;
		}
	}

	async locations() { return [EC2_LOCATION]; }
	async beacons() { return []; }
}
class MissingInstance extends Error {}
class DryRunAccepted extends Error {}
function claimTags(x: Xml): string[] {
	return x.one("tagSet").all("item").filter((t) => t.one("key").text === "Claim").map((t) => t.one("value").text);
}
