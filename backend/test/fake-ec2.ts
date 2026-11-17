// Synthetic fixtures only. Records Query fields, never the signed headers or credential values.
export const xmlResponse = (action: string, content: string) => new Response(`<?xml version="1.0"?><${action}Response xmlns="http://ec2.amazonaws.com/doc/2016-11-15/">${content}</${action}Response>`);
export const ec2Error = (code: string, status = 400) => new Response(`<Response><Errors><Error><Code>${code}</Code><Message>discard this outside text</Message></Error></Errors></Response>`, { status });
const escape = (x: string) => x.replaceAll("&", "&amp;").replaceAll("<", "&lt;");
export type Machine = { id: string; claim: string; version: string; state: string; script: string;
	purpose?: string; project?: string; workerEnv?: string; launchedAt?: number; launchTime?: string | null };

/** Query positions have no meaning: decode resource specifications and tags by their wire keys. */
export function resourceTags(q: URLSearchParams): Map<string, Record<string, string>> {
	return new Map([...q].filter(([k]) => /^TagSpecification\.\d+\.ResourceType$/.test(k)).map(([k, kind]) => {
		const prefix = k.slice(0, -"ResourceType".length);
		return [kind, Object.fromEntries([...q].filter(([key]) => key.startsWith(prefix) && key.endsWith(".Key"))
			.map(([key, value]) => [value, q.get(key.slice(0, -3) + "Value")!]))];
	}));
}
export class FakeEc2 {
	requests: URLSearchParams[] = [];
	// Fixture subnets occupy different zones, matching setup's one offering subnet per zone.
	zones = new Map([["subnet-00000000000000001", "fixture-a"], ["subnet-00000000000000002", "fixture-b"]]);
	private tokens = new Map<string, { machine: Machine; body: string }>();
	machines: Machine[] = [];
	jwks: unknown = null;
	releaseRequests = 0;
	releaseMissing = false;
	archiveMissing = false;
	releaseMalformed = false;
	errors: { code: string; status: number }[] = [];
	loseResponses = 0;
	stopPending = false;
	badXml = false;
	requestBodyHang = false;
	private tags(m: Machine) {
		return { Project: m.project ?? "overmatch", Purpose: m.purpose ?? "host", ...(m.workerEnv === undefined ? {} : { WorkerEnvironment: m.workerEnv }), Claim: m.claim, Version: m.version };
	}
	private item(m: Machine): string {
		const launch = m.launchTime === null ? "" : `<launchTime>${escape(m.launchTime ?? new Date(m.launchedAt ?? Date.now()).toISOString())}</launchTime>`;
		return `<item><instanceId>${m.id}</instanceId>${launch}<instanceState><name>${m.state}</name></instanceState><ipAddress>192.0.2.1</ipAddress><tagSet>` +
			Object.entries(this.tags(m)).map(([k, v]) => `<item><key>${k}</key><value>${escape(v)}</value></item>`).join("") + `</tagSet></item>`;
	}
	async fetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
		const url = new URL(typeof input === "string" || input instanceof URL ? input : input.url);
		if (url.hostname === "team.access.test") return new Response(JSON.stringify(this.jwks));
		if (url.hostname === "github.com") {
			this.releaseRequests++;
			const name = decodeURIComponent(url.pathname.split("/").pop()!);
			if (name.endsWith(".sha256")) return new Response(this.releaseMalformed ? "malformed" : `${"a".repeat(64)}  ${name.slice(0, -7)}\n`, { status: this.releaseMissing ? 404 : 200 });
			return new Response(null, { status: this.archiveMissing ? 404 : 200 });
		}
		if (url.hostname !== "ec2.ap-southeast-1.amazonaws.com") throw new Error("unexpected fixture endpoint");
		const q = new URLSearchParams(String(init?.body));
		this.requests.push(q);
		const action = q.get("Action")!;
		if (this.badXml) return new Response("broken");
		if (this.requestBodyHang) return new Response(new ReadableStream({ start() {} }));
		if (action === "RunInstances") {
			const error = this.errors.shift();
			if (error) return ec2Error(error.code, error.status);
			if (q.get("DryRun") === "true") return ec2Error("DryRunOperation", 412);
			const claim = q.get("ClientToken")!;
			const zone = this.zones.get(q.get("NetworkInterface.1.SubnetId")!);
			if (!zone) throw new Error("fixture must name the subnet's zone");
			const token = `${zone}:${claim}`;
			const previous = this.tokens.get(token);
			if (previous && previous.body !== q.toString()) return ec2Error("IdempotentParameterMismatch");
			let m = previous?.machine;
			if (!m) {
				const tags = resourceTags(q).get("instance")!;
				m = { id: `i-${String(this.machines.length + 1).padStart(17, "0")}`, claim: tags.Claim,
					version: tags.Version, workerEnv: tags.WorkerEnvironment, purpose: tags.Purpose, project: tags.Project,
					launchedAt: Date.now(), state: "running",
					script: new TextDecoder().decode(Uint8Array.from(atob(q.get("UserData")!), (c) => c.charCodeAt(0))) };
				this.machines.push(m);
				this.tokens.set(token, { machine: m, body: q.toString() });
			}
			if (this.loseResponses-- > 0) throw new TypeError("fixture network lost the answer");
			return xmlResponse(action, `<instancesSet><item><instanceId>${m.id}</instanceId></item></instancesSet>`);
		}
		if (action === "DescribeInstances") {
			const id = q.get("InstanceId.1");
			if (id && !this.machines.some((m) => m.id === id)) return ec2Error("InvalidInstanceID.NotFound");
			const filters = [...q].filter(([k]) => /^Filter\.\d+\.Name$/.test(k));
			const machines = this.machines.filter((m) => (!id || m.id === id) && filters.every(([k, name]) => {
				const prefix = k.slice(0, -4) + "Value.";
				const values = [...q].filter(([key]) => key.startsWith(prefix)).map(([, value]) => value);
				const actual = name === "instance-state-name" ? m.state : this.tags(m)[name.slice(4) as keyof ReturnType<FakeEc2["tags"]>];
				return actual !== undefined && values.includes(actual);
			}));
			return xmlResponse(action, `<reservationSet>${machines.map((m) => `<item><instancesSet>${this.item(m)}</instancesSet></item>`).join("")}</reservationSet>`);
		}
		if (action === "TerminateInstances") {
			const m = this.machines.find((x) => x.id === q.get("InstanceId.1"));
			if (!m) return ec2Error("InvalidInstanceID.NotFound");
			m.state = this.stopPending ? "shutting-down" : "terminated";
			return xmlResponse(action, `<instancesSet><item><instanceId>${m.id}</instanceId><currentState><name>${m.state}</name></currentState></item></instancesSet>`);
		}
		throw new Error("unexpected fixture action");
	}
	launches(): URLSearchParams[] { return this.requests.filter((q) => q.get("Action") === "RunInstances"); }
	settings(m: Machine): Record<string, string> {
		const values = Object.fromEntries([...m.script.matchAll(/^([A-Z_][A-Z0-9_]*)='([^']*)'$/gm)].map((x) => [x[1], x[2]]));
		return Object.fromEntries(atob(values.HOSTED_ENV_B64).trim().split("\n").map((line) => {
			const at = line.indexOf("="); return [line.slice(0, at), JSON.parse(line.slice(at + 1))];
		}));
	}
}
