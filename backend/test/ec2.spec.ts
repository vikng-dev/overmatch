import { abortAllDurableObjects, runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { beforeEach, describe, expect, it, vi } from "vitest";
import policy from "../../scripts/aws/host-policy.json";
import rootContract from "../../scripts/aws/host-image.json";
import production from "../production-server.json";
import hosting from "../aws-hosting.json";
import { Ec2, hostedScript } from "../src/ec2";
import { host, machineScript } from "../src/host.generated";
import { newId, tagOf } from "../src/ids";
import { adminPage } from "../src/admin";
import { exportJWK, generateKeyPair, SignJWT } from "jose";
import { config, serverEnvironment, SERVER_SETTING_DOMAINS } from "../src/config";
import { sweep } from "../src/sweep";
import { signV4 } from "../src/sigv4";
import { FakeEc2, resourceTags } from "./fake-ec2";
import { failD1Once, age, call, env, heartbeat, join, registry, ticket } from "./helpers";
import { serverRelease, RELEASE_CACHE_MS } from "../src/release";
import worker from "../src/index";
import { JOURNAL_WINDOW_MS, SERVER_LOG_MAX_BYTES } from "../src/evidence";
import { REASONS } from "../src/reasons";

let ec2: FakeEc2;
const MAX_HOSTS = hosting.maxServers;
const V = "0.8.0-fixture";
const request = (version = V) => ({ version, locations: [hosting.location.city], pings: {} });
const cfg = config(env);
const provider = () => new Ec2(env, cfg.providerTimeoutMs);

beforeEach(() => {
	ec2 = new FakeEc2();
	vi.mocked(fetch).mockImplementation((input, init) => ec2.fetch(input, init));
});

async function deadline(version = V) {
	await age(version, cfg.startDeadlineMs + cfg.statusPollMs);
	await runDurableObjectAlarm(registry(version));
}

describe("EC2 request contract", () => {
	it("sends the installed policy's constrained context explicitly on every created resource", async () => {
		await join(request());
		const q = ec2.launches()[0];
		expect(q.get("ImageId")).toBe(`resolve:ssm:${host.parameter}`);
		expect(q.get("InstanceType")).toBe(production.instance_type);
		const instance = policy.Statement.find((s) => s.Resource.endsWith?.(":*:instance/*"))!.Condition!.StringLike!;
		const context: Record<string, string | null> = {
			"ec2:InstanceType": q.get("InstanceType"), "ec2:InstanceMarketType": q.get("InstanceMarketOptions.MarketType"),
			"ec2:Tenancy": q.get("Placement.Tenancy"), "ec2:MetadataHttpTokens": q.get("MetadataOptions.HttpTokens"),
		};
		for (const [key, expected] of Object.entries(instance)) {
			if (key.startsWith("aws:RequestTag/")) continue;
			expect(context[key], key).toBe(expected === "@INSTANCE_TYPE@" ? production.instance_type : expected);
		}
		const specifications = resourceTags(q);
		expect([...specifications.keys()].sort()).toEqual(["instance", "network-interface", "volume"]);
		for (const kind of specifications.keys()) {
			const statement = policy.Statement.find((s) => s.Resource.endsWith?.(`:*:${kind}/*`))!;
			const tags = specifications.get(kind)!;
			expect(tags.WorkerEnvironment).toBe(env.WORKER_ENV);
			for (const [key, value] of Object.entries(statement.Condition!.StringLike!)) {
				if (!key.startsWith("aws:RequestTag/")) continue;
				const bound = ({ "@PROJECT@": "overmatch", "@PURPOSE@": "host" } as Record<string, string>)[value];
				expect(tags[key.slice("aws:RequestTag/".length)]).toBe(bound);
			}
			expect(tags.Claim).toBe(q.get("ClientToken"));
			expect(tags.Version).toBe(V);
		}
		const volume = policy.Statement.find((s) => s.Resource.endsWith?.(":*:volume/*"))!.Condition!;
		expect(q.get("BlockDeviceMapping.1.Ebs.VolumeType")).toBe(volume.StringLike!["ec2:VolumeType"]);
		for (const [key, cap] of Object.entries(volume.NumericLessThanEquals!)) {
			const field = key.slice("ec2:Volume".length);
			const v = Number(q.get(`BlockDeviceMapping.1.Ebs.${field === "Size" ? "VolumeSize" : field}`));
			expect(v).toBe(typeof cap === "number" ? cap : rootContract.rootSizeGiB);
		}
		expect(q.get("BlockDeviceMapping.1.DeviceName")).toBe(rootContract.rootDeviceName);
		expect(q.get("BlockDeviceMapping.1.Ebs.DeleteOnTermination")).toBe("true");
		expect(q.get("NetworkInterface.1.DeleteOnTermination")).toBe("true");
		expect(q.get("NetworkInterface.1.AssociatePublicIpAddress")).toBe("true");
		const network = JSON.parse(env.AWS_HOST_CONFIG!);
		expect(network.subnetIds).toContain(q.get("NetworkInterface.1.SubnetId"));
		expect(q.get("NetworkInterface.1.SecurityGroupId.1")).toBe(network.securityGroupId);
		expect(q.get("InstanceInitiatedShutdownBehavior")).toBe("terminate");
		expect(q.get("DisableApiTermination")).toBe("false");
		expect([...new Set(ec2.requests.map((r) => r.get("Action")))]).toEqual(["DescribeInstances", "RunInstances"]);
	});

	it.each(["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_HOST_CONFIG"] as const)("absent %s refuses before admission or an AWS request", async (missing) => {
		const id = newId(V);
		const p = new Ec2({ ...env, [missing]: undefined }, cfg.providerTimeoutMs);
		await expect(p.deploy({ version: V, city: hosting.location.city, tag: tagOf(id), clientIp: null, webhookUrl: "",
			release: { url: "https://release.example/server.tar.gz", checksum: "a".repeat(64) + "  server.tar.gz" },
			env: serverEnvironment(cfg, id, "https://worker.example", "fixture-key", "fixture-report") }))
			.rejects.toMatchObject({op: missing === "AWS_HOST_CONFIG" ? "config" : "credentials", outcome: "refused"});
		expect(ec2.requests).toHaveLength(0);
		expect(await provider().reservations()).toHaveLength(0);
	});

	it.each([
		["WORKER_ENV", "Bad Environment"], ["WORKER_ENV", "fixture\n"], ["AWS_ACCESS_KEY_ID", "fixture\r\nheader"],
		["AWS_HOST_CONFIG", "{"], ["AWS_HOST_CONFIG", JSON.stringify({ securityGroupId: "sg-00000000000000001\n", subnetIds: ["subnet-00000000000000001"] })], ["AWS_HOST_CONFIG", JSON.stringify({ securityGroupId: "invalid", subnetIds: [] })],
	])("operator %s is refused before any platform work", async (setting, value) => {
		const log = vi.spyOn(console, "error");
		const res = await worker.fetch(new Request("https://worker.example/v1/locations?version=fixture"), { ...env, [setting]: value });
		expect(res.status).toBe(503);
		expect(await res.json()).toEqual({ error: expect.stringContaining(`config: ${setting}:`) });
		expect(log).toHaveBeenCalledWith("configuration refused", expect.stringContaining(`config: ${setting}:`));
		expect(ec2.requests).toHaveLength(0);
	});

	it("EC2 consumers reuse admitted network JSON rather than reading operator settings again", () => {
		const settings = { ...env, get AWS_HOST_CONFIG() {
			if (read) throw new Error("network JSON read after startup");
			read = true;
			return env.AWS_HOST_CONFIG;
		} };
		let read = false;
		config(settings);
		expect(new Ec2(settings, cfg.providerTimeoutMs).requiresHttps).toBe(true);
	});

	it("embeds the harness boot exactly once with hosted settings, without packages or SSH", async () => {
		await join(request());
		const m = ec2.machines[0];
		const settings = ec2.settings(m);
		expect(settings.OVERMATCH_API).toBe("https://api.overmatch.test");
		expect(settings.OVERMATCH_REPORT_SECRET).toMatch(/^[a-f0-9]{64}$/);
		expect(m.script.slice(m.script.indexOf("set -Eeuo pipefail"))).toBe(machineScript.slice(machineScript.indexOf("set -Eeuo pipefail")));
		expect(m.script).toContain("ROLE='hosted'");
		const bytes = new TextEncoder().encode(m.script).byteLength;
		expect(bytes, `user data is ${bytes} bytes; shrink machine.sh or settings before exceeding EC2 16384-byte limit`).toBeLessThanOrEqual(16 * 1024);
	});

	it("worst realistic rendered user data leaves at least 1 KiB for future launch settings", async () => {
		// Fixture: maximum version/DNS lengths, a port, and full-precision accepted timings.
		const version = "0.8.0-" + "v".repeat(64 - "0.8.0-".length);
		const origin = `https://${[63, 63, 63, 61].map((n) => "a".repeat(n)).join(".")}:65535`;
		const id = newId(version);
		const settings = { ...cfg, capacity: SERVER_SETTING_DOMAINS.CAPACITY.max,
			heartbeatIntervalMs: 12345.678901234567 * 1000,
			emptyExitS: 12345.678901234567, serverStatsS: 0.0010000000000000002 };
		const script = hostedScript({ version, city: hosting.location.city, tag: tagOf(id), clientIp: null, webhookUrl: "",
			release: await serverRelease(version, cfg.providerTimeoutMs),
			env: serverEnvironment(settings, id, origin, "a".repeat(64), "b".repeat(64)) });
		const bytes = new TextEncoder().encode(script).byteLength;
		const headroom = 16 * 1024 - bytes;
		console.warn(`Rendered worst realistic user data: ${bytes} bytes; ${headroom} bytes headroom`);
		expect(bytes, `${bytes} bytes exceeds EC2 16384-byte limit; shrink machine.sh or settings`).toBeLessThanOrEqual(16 * 1024);
		expect(headroom, `${bytes} bytes leaves ${headroom} bytes; shrink embedded boot code or settings`).toBeGreaterThanOrEqual(1024);
	});

	it("matches AWS's published IAM ListUsers SigV4 vector (2015-08-30)", async () => {
		// Public AWS example credentials are fixture data. AWS General Reference's SigV4 worked example.
		const signed = await signV4({ method: "GET", url: new URL("https://iam.amazonaws.com/?Action=ListUsers&Version=2010-05-08"), body: "",
			headers: { "content-type": "application/x-www-form-urlencoded; charset=utf-8" }, accessKey: "fixture-access",
			secretKey: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", region: "us-east-1", service: "iam", date: new Date("2015-08-30T12:36:00Z") });
		expect(signed.authorization.split("Signature=")[1]).toBe("5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7");
	});
	it("matches AWS's published S3 GET object with Range SigV4 vector (2013-05-24)", async () => {
		// AWS S3 API, sig-v4-header-based-auth.html, Example: GET Object. Public fixture key.
		const signed = await signV4({ method: "GET", url: new URL("https://examplebucket.s3.amazonaws.com/test.txt"), body: "",
			headers: { range: "bytes=0-9", "x-amz-content-sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" },
			accessKey: "fixture-access", secretKey: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", region: "us-east-1", service: "s3", date: new Date("2013-05-24T00:00:00Z") });
		expect(signed.authorization.split("Signature=")[1]).toBe("f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41");
	});
	it("matches AWS's POST form-body SigV4 vector, hashing the exact transmitted bytes", async () => {
		// AWS test suite as distributed by botocore: tests/unit/auth/aws4_testsuite/post-x-www-form-urlencoded.
		const signed = await signV4({ method: "POST", url: new URL("https://example.amazonaws.com/"), body: "Param1=value1",
			headers: { "content-type": "application/x-www-form-urlencoded" }, accessKey: "fixture-access",
			secretKey: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", region: "us-east-1", service: "service", date: new Date("2015-08-30T12:36:00Z") });
		expect(signed.authorization.split("Signature=")[1]).toBe("ff11897932ad3f4e8b18135d722051e5ac45fc38421b1da7b9d196a0fe09473a");
	});

});

describe("EC2 lifecycle behind the shipped proving Worker", () => {
	it("reserves at most the owner's cap across concurrent versions and object eviction", async () => {
		const tickets = await Promise.all(Array.from({ length: MAX_HOSTS + 1 }, (_, i) => join(request(`0.8.${i}-fixture`))));
		expect(ec2.machines.length).toBe(MAX_HOSTS);
		const answers = await Promise.all(tickets.map(ticket));
		expect(answers.filter((t) => t.reason === REASONS.noCapacity)).toHaveLength(1);
		expect(await provider().reservations()).toHaveLength(MAX_HOSTS);
		await abortAllDurableObjects();
		expect((await ticket(await join(request("0.8.99-fixture"))))).toMatchObject({state: "failed", reason: REASONS.noCapacity});
		expect(ec2.machines).toHaveLength(MAX_HOSTS);
	});

	it("closed matches occupy their slots while a new same-version player is refused at the cap", async () => {
		for (let i = 0; i < MAX_HOSTS; i++) {
			await join(request());
			const s = ec2.settings(ec2.machines[i]);
			await heartbeat(s.OVERMATCH_SERVER_ID, s.OVERMATCH_REPORT_SECRET, {players: 1, capacity: cfg.capacity, state: "closed"});
		}
		const t = await join(request());
		expect(await ticket(t)).toMatchObject({state: "failed", reason: REASONS.noCapacity});
		expect(ec2.machines).toHaveLength(MAX_HOSTS);
	});

	it.each(["RunInstances", "release"])("an internal %s exception fails the join instead of retaining an unknown claim", async (operation) => {
		vi.mocked(fetch).mockImplementation((input, init) => {
			const url = new URL(typeof input === "string" || input instanceof URL ? input : input.url);
			if ((operation === "release" && url.hostname === "github.com")
				|| (operation === "RunInstances" && new URLSearchParams(String(init?.body)).get("Action") === operation)) throw new Error("fixture adapter invariant");
			return ec2.fetch(input, init);
		});
		await expect(join(request())).rejects.toThrow("fixture adapter invariant");
	});

	it.each(["{", '{"url":null,"checksum":42}'])("old cached release JSON %s is refused visibly before launch", async (raw) => {
		await runInDurableObject(registry(V), (_, state) => { state.storage.sql.exec("INSERT INTO catalog VALUES (?, ?, ?)", `release:${V}`, raw, Date.now()); });
		const response = await call("POST", "/v1/join", request());
		expect(response.status).toBe(503);
		expect(await response.json()).toEqual({ error: REASONS.releaseUnavailable });
		expect(ec2.requests).toHaveLength(0);
	});

	it("same-version concurrent joins share one launch and one cached release fetch", async () => {
		const tickets = await Promise.all([join(request()), join(request())]);
		expect(ec2.machines).toHaveLength(1);
		expect(ec2.releaseRequests).toBe(2); // One checksum GET plus one archive HEAD.
		expect(await Promise.all(tickets.map(ticket))).toEqual([{ state: "starting" }, { state: "starting" }]);
		await abortAllDurableObjects();
		await join(request());
		expect(ec2.releaseRequests).toBe(2);
		expect(ec2.machines).toHaveLength(1);
	});

	it("promotes only after address and authenticated heartbeat; match end confirms termination", async () => {
		const t = await join(request());
		const m = ec2.machines[0], s = ec2.settings(m);
		expect(await ticket(t)).toEqual({ state: "starting" });
		expect((await heartbeat(s.OVERMATCH_SERVER_ID, s.OVERMATCH_REPORT_SECRET, { players: 2, capacity: cfg.capacity, state: "open" })).status).toBe(204);
		expect(await ticket(t)).toMatchObject({ state: "ready", address: "192.0.2.1", port: host.port, key: s.OVERMATCH_NETCODE_KEY });
		expect((await call("POST", `/v1/servers/${s.OVERMATCH_SERVER_ID}/end`, {}, { authorization: `Bearer ${s.OVERMATCH_REPORT_SECRET}` })).status).toBe(204);
		expect(m.state).toBe("running"); // The process must receive its reply before its stop hook can push.
		await age(V, provider().logs.exitGraceMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(m.state).toBe("terminated");
		expect(await provider().reservations()).toHaveLength(0);
	});

	it("silent boots wait for confirmed termination, preserve location and exhaust the owner's budget", async () => {
		const tickets = [await join(request())];
		for (let attempt = 0; attempt < hosting.startAttempts; attempt++) {
			ec2.stopPending = true;
			await deadline();
			expect(ec2.machines).toHaveLength(attempt + 1);
			expect(await ticket(tickets[0])).toEqual({ state: "starting" });
			ec2.stopPending = false;
			await age(V, cfg.statusPollMs + 1);
			await runDurableObjectAlarm(registry(V));
			expect(ec2.machines[attempt].state).toBe("terminated");
			if (attempt + 1 < hosting.startAttempts) {
				expect(ec2.machines).toHaveLength(attempt + 2);
				// A late join inherits the current boot's budget, so it cannot extend the chain.
				tickets.push(await join(request()));
			}
		}
		expect(ec2.machines).toHaveLength(hosting.startAttempts);
		for (const id of tickets) expect(await ticket(id)).toEqual({ state: "failed", reason: REASONS.startTimeout });
		expect(await provider().reservations()).toHaveLength(0);
	});

	it("a boot-step end report uses the same single fresh-machine budget", async () => {
		const t = await join(request());
		for (let i = 0; i < hosting.startAttempts; i++) {
			const s = ec2.settings(ec2.machines[i]);
			await call("POST", `/v1/servers/${s.OVERMATCH_SERVER_ID}/end`, { reason: "boot-verify", journalAvailable: false }, { authorization: `Bearer ${s.OVERMATCH_REPORT_SECRET}` });
			expect(ec2.machines[i].state).toBe("terminated");
		}
		expect(ec2.machines).toHaveLength(hosting.startAttempts);
		expect((await ticket(t)).state).toBe("failed");
	});

	it("a lost RunInstances answer retries identical bytes and token, creating one machine", async () => {
		ec2.loseResponses = 1;
		await join(request());
		expect(ec2.launches()).toHaveLength(2);
		expect(ec2.launches()[0].toString()).toBe(ec2.launches()[1].toString());
		expect(ec2.machines).toHaveLength(1);
	});

	it("two lost answers retain the claim; alarm lookup adopts it without another launch", async () => {
		ec2.loseResponses = 2;
		const t = await join(request());
		await age(V, cfg.statusPollMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(ec2.machines).toHaveLength(1);
		expect(ec2.launches()).toHaveLength(2);
		expect((await registry(V).snapshot())[0].deployment_id).toBe(ec2.machines[0].id);
		expect(await ticket(t)).toEqual({ state: "starting" });
	});

	it("sweeps orphan machines and orphan slots, preserving a live version's machine", async () => {
		await join(request());
		const live = ec2.machines[0];
		const orphanId = newId("0.8.1-fixture");
		const orphan = { id: "i-00000000000000999", claim: tagOf(orphanId), version: "0.8.1-fixture", state: "running", script: "", workerEnv: env.WORKER_ENV };
		ec2.machines.push(orphan);
		await runInDurableObject(env.FLEET.getByName("ec2"), (_, state) => { state.storage.sql.exec("INSERT INTO slots VALUES (?, ?)", orphanId, 0); });
		const result = await sweep(env, Date.now() + cfg.startDeadlineMs + 3 * cfg.providerTimeoutMs + 1);
		expect(result.stopped).toBe(1);
		expect(orphan.state).toBe("terminated");
		expect(live.state).toBe("running");
		expect((await provider().reservations()).map((slot) => slot.server_id)).toEqual([ec2.settings(live).OVERMATCH_SERVER_ID]);
	});

	it.each(["checksum", "archive"])("a missing %s refuses before tickets, admission or any AWS request", async (what) => {
		ec2.releaseMissing = what === "checksum"; ec2.archiveMissing = what === "archive";
		const res = await call("POST", "/v1/join", request());
		expect(res.status).toBe(404);
		expect(await res.json()).toEqual({ error: REASONS.releaseMissing });
		expect(ec2.requests).toHaveLength(0);
		expect(await provider().reservations()).toHaveLength(0);
	});

	it("lost heartbeats terminate a ready host and return its slot", async () => {
		await join(request());
		const s = ec2.settings(ec2.machines[0]);
		await heartbeat(s.OVERMATCH_SERVER_ID, s.OVERMATCH_REPORT_SECRET, {players: 1, capacity: cfg.capacity, state: "open"});
		await age(V, cfg.heartbeatDeadlineMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(ec2.machines[0].state).toBe("terminated");
		expect(await provider().reservations()).toHaveLength(0);
	});

	it("tagged machines lacking version or claim tags are swept too, never hidden from the cap", async () => {
		ec2.machines.push(...Array.from({ length: MAX_HOSTS }, (_, i) => ({
			id: `i-${String(i + 91).padStart(17, "0")}`, claim: "", version: i % 2 ? V : "", state: "running", script: "", workerEnv: env.WORKER_ENV,
		})));
		const t = await join(request());
		expect((await ticket(t)).reason).toBe(REASONS.noCapacity);
		expect(ec2.launches()).toHaveLength(0);
		expect(await sweep(env)).toMatchObject({ listed: ec2.machines.length, stopped: ec2.machines.length });
		expect(ec2.machines.every((m) => m.state === "terminated")).toBe(true);
	});

	it("inventory excludes harness and foreign environments from admission and sweeping", async () => {
		const foreign = Array.from({ length: MAX_HOSTS + 1 }, (_, i) => ({
			id: `i-${String(i + 70).padStart(17, "0")}`, claim: "", version: "", state: "running", script: "",
			purpose: i % 2 ? "host" : "harness", workerEnv: i % 2 ? "other-worker" : env.WORKER_ENV,
		}));
		ec2.machines.push(...foreign);
		await join(request());
		expect(await provider().deployments(1)).toMatchObject({ items: [{ id: ec2.machines.at(-1)!.id }], truncated: false });
		expect(await sweep(env)).toMatchObject({ listed: 1, stopped: 0 });
		expect(foreign.every((m) => m.state === "running")).toBe(true);
		expect(ec2.launches()).toHaveLength(1);
	});

	it("a foreign Worker can neither see nor sweep this Worker's machine", async () => {
		await join(request());
		const other = { ...env, WORKER_ENV: "other-worker" };
		expect((await new Ec2(other, cfg.providerTimeoutMs).deployments(1)).items).toEqual([]);
		expect(await sweep(other)).toMatchObject({ listed: 0, stopped: 0 });
		expect(ec2.machines[0].state).toBe("running");
	});

	it("the independent age guard ends even a currently heartbeating live claim", async () => {
		await join(request());
		const m = ec2.machines[0], settings = ec2.settings(m), now = Date.now();
		const maxAge = (hosting.lifetimeMinutes + hosting.sweepMarginMinutes) * 60_000;
		m.launchedAt = now - maxAge + 1;
		await heartbeat(settings.OVERMATCH_SERVER_ID, settings.OVERMATCH_REPORT_SECRET, {players: 1, capacity: cfg.capacity, state: "open"});
		expect(await sweep(env, now)).toMatchObject({ stopped: 0 });
		expect(await sweep(env, now + 2)).toMatchObject({ stopped: 1 });
		expect(m.state).toBe("terminated");
	});

	it("Fleet refuses malformed server ids before inventory or persistence", async () => {
		await runInDurableObject(env.FLEET.getByName("ec2"), async (fleet) => {
			await expect(fleet.reserve("malformed-fixture")).rejects.toThrow("versioned server id");
		});
		expect(ec2.requests).toHaveLength(0);
		expect(await provider().reservations()).toEqual([]);
	});

	it("a corrupt persisted Fleet id fails the sweep loudly instead of holding a hidden reservation", async () => {
		await runInDurableObject(env.FLEET.getByName("ec2"), (_, state) => {
			state.storage.sql.exec("INSERT INTO slots VALUES (?, ?)", "malformed-fixture", 0);
		});
		await expect(sweep(env)).rejects.toThrow("versioned server id");
	});

	it("a missing Fleet table fails the invocation instead of being retried as storage downtime", async () => {
		await runInDurableObject(env.FLEET.getByName("ec2"), (_, state) => { state.storage.sql.exec("DROP TABLE slots"); });
		await expect(sweep(env)).rejects.toThrow("no such table: slots");
		expect(await env.DB.prepare("SELECT failed FROM sweep WHERE id = 1").first()).toBeNull();
		await abortAllDurableObjects();
		expect(await sweep(env)).toMatchObject({ failed: 0 });
	});

	it("failed Fleet release retains a stopping row and tickets across eviction, then retries durably", async () => {
		const t = await join(request());
		await runInDurableObject(env.FLEET.getByName("ec2"), (fleet, state) => {
			const release = fleet.release.bind(fleet);
			const prototype: typeof fleet = Object.getPrototypeOf(fleet);
			const releaseFault = vi.spyOn(prototype, "release").mockImplementation((serverId: string) => {
				const fault = vi.spyOn(state.storage.sql, "exec").mockImplementation(() => {
					throw Object.assign(new Error("fixture transient storage fault"), { retryable: true });
				});
				try { return release(serverId); }
				finally { fault.mockRestore(); releaseFault.mockRestore(); }
			});
		});
		const log = vi.spyOn(console, "error");
		await deadline();
		expect((await registry(V).snapshot())[0].state).toBe("stopping");
		expect(await provider().reservations()).toHaveLength(1);
		expect(await ticket(t)).toEqual({ state: "starting" });
		expect(log).toHaveBeenCalledWith("Fleet release failed; stopping row retains retry", expect.stringContaining("fleetRelease:storage"));
		await abortAllDurableObjects();
		await age(V, cfg.statusPollMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect((await registry(V).snapshot())[0].state).toBe("ended");
		// A waiting player's replacement may reserve one new slot, never the stopped claim's slot.
		expect((await provider().reservations()).some((s) => s.server_id === ec2.settings(ec2.machines[0]).OVERMATCH_SERVER_ID)).toBe(false);
	});

	it.each([null, "unreadable-fixture"])("unreadable launch time %s counts toward admission but preserves per-host orphan checks", async (launchTime) => {
		ec2.machines.push(...Array.from({ length: MAX_HOSTS }, (_, i) => ({
			id: `i-${String(i + 91).padStart(17, "0")}`, claim: "", version: V, state: "running", script: "",
			workerEnv: env.WORKER_ENV, launchTime: i === 0 ? launchTime : undefined,
		})));
		expect((await ticket(await join(request()))).reason).toBe(REASONS.noCapacity);
		const log = vi.spyOn(console, "error");
		expect(await sweep(env)).toMatchObject({ listed: MAX_HOSTS, stopped: MAX_HOSTS, failed: 1, error: expect.stringContaining("launchTime") });
		expect(log).toHaveBeenCalledWith("sweep", expect.stringContaining(`host ${ec2.machines[0].id} launch time unreadable`));
	});

	it("an unreadable launch time still receives the live ownership check", async () => {
		await join(request());
		ec2.machines[0].launchTime = null;
		expect(await sweep(env)).toMatchObject({ listed: 1, stopped: 0, failed: 1 });
		expect(ec2.machines[0].state).toBe("running");
	});

	it("an untagged Purpose=host machine is invisible to this Worker, requiring proof step 0", async () => {
		ec2.machines.push({ id: "i-00000000000000099", claim: "", version: V, state: "running", script: "" });
		await join(request());
		expect(ec2.launches()).toHaveLength(1);
		expect(await sweep(env)).toMatchObject({ listed: 1, stopped: 0 });
		expect(ec2.machines[0].state).toBe("running");
	});

	it("Registry lifetime expiry releases a live row, tickets and slot despite fresh heartbeats", async () => {
		const t = await join(request());
		const s = ec2.settings(ec2.machines[0]);
		await heartbeat(s.OVERMATCH_SERVER_ID, s.OVERMATCH_REPORT_SECRET, {players: 1, capacity: cfg.capacity, state: "open"});
		await runInDurableObject(registry(V), (_, state) => {
			state.storage.sql.exec("UPDATE servers SET created_at = ?", Date.now() - provider().inventory.maxAgeMs - 1);
		});
		await abortAllDurableObjects();
		await runDurableObjectAlarm(registry(V));
		expect((await registry(V).snapshot())[0]).toMatchObject({ state: "ended", reason: REASONS.lifetimeExpired });
		expect(await provider().reservations()).toHaveLength(0);
		expect(await ticket(t)).toEqual({ state: "failed", reason: REASONS.lifetimeExpired });
		expect((await heartbeat(s.OVERMATCH_SERVER_ID, s.OVERMATCH_REPORT_SECRET, {players: 1, capacity: cfg.capacity, state: "open"})).status).toBe(410);
	});

	it("refreshes cached hits after the same bounded lifetime as misses, including eviction", async () => {
		await join(request());
		await abortAllDurableObjects();
		ec2.releaseMalformed = true;
		expect((await call("POST", "/v1/join", request())).status).toBe(200);
		await runInDurableObject(registry(V), (_, state) => {
			state.storage.sql.exec("UPDATE catalog SET fetched_at = fetched_at - ?", RELEASE_CACHE_MS + 1);
		});
		expect((await call("POST", "/v1/join", request())).status).toBe(503);
		expect(ec2.launches()).toHaveLength(1);
	});

	it("refuses unverified GitHub build-metadata names before release reads, tickets or admission", async () => {
		const res = await call("POST", "/v1/join", request("0.8.0+fixture"));
		expect(res.status).toBe(400);
		expect(await res.json()).toEqual({ error: REASONS.releaseVersionUnsupported });
		expect(ec2.releaseRequests).toBe(0);
		expect(ec2.requests).toHaveLength(0);
	});

	it("refuses plaintext origins before creating a ticket, slot or release lookup", async () => {
		const response = await registry(V).join(V, [hosting.location.city], {}, null, "http://worker.example");
		expect(response).toMatchObject({ok: false, status: 400, error: expect.stringContaining("insecureOrigin")});
		expect(ec2.requests).toHaveLength(0);
		expect(ec2.releaseRequests).toBe(0);
		expect(await provider().reservations()).toHaveLength(0);
	});

	it("the fake preserves zonal token idempotency and detects changed parameters", async () => {
		await join(request());
		const same = new URLSearchParams(ec2.launches()[0]);
		const endpoint = "https://ec2.ap-southeast-1.amazonaws.com/";
		await ec2.fetch(endpoint, {body: same.toString()});
		expect(ec2.machines).toHaveLength(1);
		same.set("InstanceType", "fixture-other-type");
		expect(await (await ec2.fetch(endpoint, {body: same.toString()})).text()).toContain("IdempotentParameterMismatch");
		same.set("NetworkInterface.1.SubnetId", [...ec2.zones.keys()][1]);
		await ec2.fetch(endpoint, {body: same.toString()});
		expect(ec2.machines).toHaveLength(2);
	});

	it("EC2 admin cost remains explicitly unknown rather than applying Edgegap's price", async () => {
		const html = await adminPage(env);
		expect(html).toContain("EC2 cost is unknown here");
		expect(html).toContain("Fleet: 0 reservations");
		expect(html).not.toContain("Edgegap invoice");
		expect(html).not.toContain("$0.00");
	});

	it("caches a missing release briefly, then allows a subsequently published version", async () => {
		ec2.releaseMissing = true;
		for (let i = 0; i < 2; i++) expect((await call("POST", "/v1/join", request())).status).toBe(404);
		expect(ec2.releaseRequests).toBe(1);
		ec2.releaseMissing = false;
		await runInDurableObject(registry(V), (_, state) => {
			state.storage.sql.exec("UPDATE catalog SET fetched_at = fetched_at - ?", RELEASE_CACHE_MS + 1);
		});
		expect((await call("POST", "/v1/join", request())).status).toBe(200);
		expect(ec2.machines).toHaveLength(1);
	});

	it("a malformed checksum refuses visibly without launching", async () => {
		ec2.releaseMalformed = true;
		const res = await call("POST", "/v1/join", request());
		expect(res.status).toBe(503);
		expect(await res.json()).toEqual({ error: REASONS.releaseUnavailable });
		expect(ec2.requests).toHaveLength(0);
	});
});

describe("AWS error codes own the outcome, independent of HTTP status", () => {
	it.each(["InsufficientInstanceCapacity", "InsufficientHostCapacity", "InstanceLimitExceeded", "VcpuLimitExceeded", "InsufficientFreeAddressesInSubnet"])("%s tries every subnet then refuses capacity", async (code) => {
		const subnets: string[] = JSON.parse(env.AWS_HOST_CONFIG!).subnetIds;
		ec2.errors = subnets.map(() => ({ code, status: 503 }));
		const t = await join(request());
		expect((await ticket(t)).reason).toBe(REASONS.noCapacity);
		expect(ec2.launches().map((q) => q.get("NetworkInterface.1.SubnetId"))).toEqual(subnets);
		expect(ec2.machines).toHaveLength(0);
	});

	it("capacity refusal in one subnet proceeds to the next offering subnet", async () => {
		ec2.errors = [{ code: "InsufficientInstanceCapacity", status: 500 }];
		await join(request());
		expect(ec2.launches()).toHaveLength(2);
		expect(ec2.machines).toHaveLength(1);
	});

	it.each(["UnauthorizedOperation", "AuthFailure", "InvalidClientTokenId", "SignatureDoesNotMatch", "RequestExpired", "MissingAuthenticationToken", "InvalidParameter", "InvalidParameterValue", "InvalidParameterCombination", "MissingParameter", "Unsupported", "UnsupportedOperation", "InvalidAMIID.NotFound", "InvalidAMIID.Unavailable", "InvalidAMIID.Malformed", "InvalidBlockDeviceMapping", "InvalidSubnetID.NotFound", "InvalidSubnetID.Malformed", "InvalidGroup.NotFound", "InvalidGroupId.Malformed", "InvalidKeyPair.NotFound", "InvalidInstanceID.Malformed", "OperationNotPermitted", "Blocked", "PendingVerification"])("%s refuses once with no cross-provider fallback", async (code) => {
		ec2.errors = [{ code, status: 503 }];
		const t = await join(request());
		expect((await ticket(t)).reason).toBe(REASONS.startRefused);
		expect(ec2.launches()).toHaveLength(1);
	});

	it.each(["InternalError", "InternalFailure", "Unavailable", "ServiceUnavailable", "RequestLimitExceeded", "Throttling", "ThrottlingException", "RequestTimeout", "FutureUnknownCode", "IdempotentParameterMismatch"])("%s preserves unknown fate and retries the same launch", async (code) => {
		ec2.errors = [{ code, status: 400 }];
		await join(request());
		expect(ec2.launches()).toHaveLength(2);
		expect(ec2.launches()[0].toString()).toBe(ec2.launches()[1].toString());
		expect(ec2.machines).toHaveLength(1);
	});

	it("repeated idempotent-parameter mismatch retains an unknown claim and slot for reconciliation", async () => {
		ec2.errors = Array.from({length: 2}, () => ({code: "IdempotentParameterMismatch", status: 400}));
		const t = await join(request());
		expect(await ticket(t)).toEqual({state: "starting"});
		expect(await provider().reservations()).toHaveLength(1);
		await age(V, cfg.statusPollMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(await ticket(t)).toEqual({state: "starting"});
		expect(ec2.launches()).toHaveLength(2);
	});

	it("NotFound keeps a new instance unknown until the registry's visibility deadline, then accepts absence", async () => {
		await expect(provider().status("i-00000000000000099", Date.now() + cfg.startDeadlineMs)).rejects.toMatchObject({outcome: "unknown"});
		await expect(provider().stop("i-00000000000000099", Date.now() + cfg.startDeadlineMs)).rejects.toMatchObject({outcome: "unknown"});
		expect((await provider().status("i-00000000000000099", 0)).phase).toBe("gone");
		await expect(provider().stop("i-00000000000000099", 0)).resolves.toBeUndefined();
	});

	it("malformed XML and stalled response bodies remain typed unknown outcomes", async () => {
		ec2.badXml = true;
		await expect(provider().status("i-00000000000000099")).rejects.toMatchObject({ outcome: "unknown" });
		ec2.badXml = false; ec2.requestBodyHang = true;
		await expect(provider().status("i-00000000000000099")).rejects.toMatchObject({ outcome: "unknown" });
	});
});


describe("operator-only provider-key authorization proof", () => {
	const keys = generateKeyPair("RS256");
	async function authorization() {
		const pair = await keys;
		ec2.jwks = { keys: [{ ...(await exportJWK(pair.publicKey)), kid: "proof", alg: "RS256", use: "sig" }] };
		return { "cf-access-jwt-assertion": await new SignJWT({})
			.setProtectedHeader({alg: "RS256", kid: "proof"}).setIssuer(cfg.access!.issuer).setAudience(cfg.access!.audience)
			.setIssuedAt().setExpirationTime("5m").sign(pair.privateKey) };
	}
	it("requires Access before reading releases or calling AWS", async () => {
		expect((await call("POST", "/admin/aws-proof", {version: V})).status).toBe(403);
		expect(ec2.requests).toHaveLength(0);
		expect(ec2.releaseRequests).toBe(0);
	});
	it.each([undefined, "ami-00000000000000001"])("proves native or concrete image authorization in every allowed subnet without creating anything", async (imageId) => {
		const res = await call("POST", "/admin/aws-proof", {version: V, imageId}, await authorization());
		expect(res.status).toBe(200);
		expect(await res.json()).toMatchObject({proof: "authorized", creates: false});
		const subnets: string[] = JSON.parse(env.AWS_HOST_CONFIG!).subnetIds;
		expect(ec2.requests.map((q) => q.get("NetworkInterface.1.SubnetId"))).toEqual(subnets);
		expect(ec2.requests.every((q) => q.get("Action") === "RunInstances" && q.get("DryRun") === "true")).toBe(true);
		expect(ec2.machines).toHaveLength(0);
		expect(await provider().reservations()).toHaveLength(0);
	});
	it("a proof's missing-location invariant fails the request instead of refusing authorization", async () => {
		const locations = vi.spyOn(Ec2.prototype, "locations").mockResolvedValue([]);
		await expect(call("POST", "/admin/aws-proof", {version: V}, await authorization())).rejects.toThrow("at least one provider location");
		locations.mockRestore();
		expect(ec2.requests).toHaveLength(0);
	});

	it("an authorization refusal is visible and is never counted as a passed proof", async () => {
		ec2.errors = [{code: "UnauthorizedOperation", status: 403}];
		expect((await call("POST", "/admin/aws-proof", {version: V}, await authorization())).status).toBe(503);
		expect(ec2.machines).toHaveLength(0);
	});
});


describe("machine journal admission and operator archive", () => {
	it("the machine and Worker agree on the byte admission contract", () => {
		expect(Number(/JOURNAL_MAX_BYTES=(\d+)/.exec(machineScript)![1])).toBe(SERVER_LOG_MAX_BYTES);
	});
	async function server() {
		await join(request());
		const machine = ec2.machines[0];
		const values = ec2.settings(machine);
		return { machine, OVERMATCH_SERVER_ID: values.OVERMATCH_SERVER_ID,
			OVERMATCH_REPORT_SECRET: values.OVERMATCH_REPORT_SECRET, OVERMATCH_NETCODE_KEY: values.OVERMATCH_NETCODE_KEY };
	}
	async function push(s: { OVERMATCH_SERVER_ID: string; OVERMATCH_REPORT_SECRET: string }, body: BodyInit = "last diagnostic", headers: Record<string, string> = {}) {
		return worker.fetch(new Request(`https://api.overmatch.test/v1/servers/${s.OVERMATCH_SERVER_ID}/journal`, {
			method: "POST", headers: { authorization: `Bearer ${s.OVERMATCH_REPORT_SECRET}`, "x-journal-truncated": "0", ...headers }, body,
		}), env);
	}
	async function archive(id: string) {
		// The README query, with only its provider identity substituted; operators rely on this join.
		return env.DB.prepare("SELECT crash, log, error FROM server_logs LEFT JOIN server_crashes USING (server_id) WHERE deployment_id = ?1").bind(id).first();
	}
	it.each([false, true])("the operator reads the final journal after a clean/crash=%s report before fallback termination", async (crash) => {
		const s = await server();
		await heartbeat(s.OVERMATCH_SERVER_ID, s.OVERMATCH_REPORT_SECRET, { players: 0, capacity: cfg.capacity, state: "open" });
		const end = await call("POST", `/v1/servers/${s.OVERMATCH_SERVER_ID}/end`, crash ? { crash: "panic fixture" } : { reason: "empty" }, { authorization: `Bearer ${s.OVERMATCH_REPORT_SECRET}` });
		expect(end.status).toBe(204);
		expect(s.machine.state).toBe("running");
		expect(ec2.requests.filter((q) => q.get("Action") === "TerminateInstances")).toHaveLength(0);
		expect((await push(s, "final diagnostic")).status).toBe(204);
		expect(await archive(s.machine.id)).toEqual({ crash: crash ? "panic fixture" : null, log: "final diagnostic", error: null });
		await abortAllDurableObjects(); // The grace and capture survive eviction.
		await age(V, provider().logs.exitGraceMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(s.machine.state).toBe("terminated");
		expect(await archive(s.machine.id)).toMatchObject({ log: "final diagnostic" });
	});
	it.each(["wrong", "missing", "unknown"])("outside %s identity is visibly refused before reading a body or storing an archive", async (kind) => {
		const s = await server();
		let read = false;
		const body = new ReadableStream<Uint8Array>({ pull() { read = true; throw new Error("unauthorized body read"); } }, { highWaterMark: 0 });
		const warn = vi.spyOn(console, "warn");
		const res = await push({ ...s, OVERMATCH_SERVER_ID: kind === "unknown" ? newId(V) : s.OVERMATCH_SERVER_ID }, body,
			{ authorization: kind === "missing" ? "" : `Bearer ${kind === "wrong" ? "wrong" : s.OVERMATCH_REPORT_SECRET}` });
		expect(res.status).toBe(kind === "unknown" ? 410 : 401);
		expect(read).toBe(false);
		expect(warn).toHaveBeenCalledWith("journal refused", res.status, expect.any(String));
		expect(await archive(s.machine.id)).toBeNull();
		warn.mockRestore();
	});
	it("leaked own credential encodings are replaced with a row-only warning and keep the archive", async () => {
		const s = await server();
		const hex = [s.OVERMATCH_REPORT_SECRET.toLowerCase(), s.OVERMATCH_REPORT_SECRET.toUpperCase()];
		const forms = [...hex.flatMap((v) => [v, "0x" + v, "0X" + v, btoa(v)]),
			btoa(String.fromCharCode(...s.OVERMATCH_REPORT_SECRET.match(/../g)!.map((b) => parseInt(b, 16))))];
		const warn = vi.spyOn(console, "warn");
		expect((await push(s, forms.join("\n") + "\nlast diagnostic")).status).toBe(204);
		expect(warn).toHaveBeenCalledWith("journal credential replaced", s.OVERMATCH_SERVER_ID);
		const saved = JSON.stringify(await archive(s.machine.id));
		for (const value of forms) expect(saved).not.toContain(value);
		expect(saved).toContain("last diagnostic");
		expect(warn.mock.calls.flat().join(" ")).not.toContain(s.OVERMATCH_REPORT_SECRET);
		warn.mockRestore();
	});
	it("concurrent second pushes cannot replace the operator's first archive", async () => {
		const s = await server();
		const results = await Promise.all([push(s, "first candidate"), push(s, "second candidate")]);
		expect(results.map((r) => r.status).sort()).toEqual([204, 409]);
		const saved = await archive(s.machine.id);
		expect((await push(s, "replacement")).status).toBe(409);
		expect(await archive(s.machine.id)).toEqual(saved);
	});
	it("the operator cannot store an unbounded body with either a claimed length or a chunked stream", async () => {
		const s = await server();
		let reads = 0;
		const body = () => new ReadableStream<Uint8Array>({
			pull(controller) { reads++; controller.enqueue(new Uint8Array(SERVER_LOG_MAX_BYTES / 2 + 1).fill(120)); },
		}, { highWaterMark: 0 });
		expect((await push(s, body(), { "content-length": String(SERVER_LOG_MAX_BYTES + 1) })).status).toBe(413);
		expect(reads).toBe(0);
		const warn = vi.spyOn(console, "warn");
		expect((await push(s, body())).status).toBe(413);
		expect(reads).toBe(2);
		expect(warn).toHaveBeenCalledWith("request refused", "journalSize");
		expect(await archive(s.machine.id)).toBeNull();
		warn.mockRestore();
	});
	it("expired server credentials cannot create late archives for the operator", async () => {
		const s = await server();
		await age(V, cfg.startDeadlineMs + JOURNAL_WINDOW_MS + 1);
		expect((await push(s)).status).toBe(410);
		expect(await archive(s.machine.id)).toBeNull();
	});
	it("a stopping row's admission deadline cannot slide forward with stop retries", async () => {
		const s = await server();
		await call("POST", `/v1/servers/${s.OVERMATCH_SERVER_ID}/end`, {}, { authorization: `Bearer ${s.OVERMATCH_REPORT_SECRET}` });
		await age(V, JOURNAL_WINDOW_MS + 1);
		ec2.stopPending = true;
		await runDurableObjectAlarm(registry(V));
		expect((await push(s)).status).toBe(410);
	});
	it("a spontaneous process exit can archive without an end report, including after provider termination", async () => {
		const s = await server();
		await heartbeat(s.OVERMATCH_SERVER_ID, s.OVERMATCH_REPORT_SECRET, { players: 0, capacity: cfg.capacity, state: "open" });
		await age(V, cfg.heartbeatDeadlineMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(s.machine.state).toBe("terminated");
		expect((await push(s)).status).toBe(204);
	});
	it("each machine redaction form is also sanitized at the durable-storage edge for the operator", async () => {
		const s = await server();
		const patterns = [s.OVERMATCH_NETCODE_KEY, "private_key=[1, 2]", "netcode-key=[3, 4]", "password=value", "token=value",
			"secret=value", "Authorization: Bearer other", "env_vars={}", "environment={}", "OVERMATCH_API=value", "ticket=value",
			"MC44LjA." + "b".repeat(32), "https://worker.example/path", "http://example/path", "192.0.2.1", "2001:db8::1", "::1", "::ffff:192.0.2.1", "fe80::1%eth0", "peer:2001:db8::1", "0x" + "a".repeat(64), "k" + "b".repeat(64), JSON.stringify(Array.from({ length: 32 }, (_, i) => i))];
		expect((await push(s, patterns.join("\n") + "\nlast diagnostic", { "x-journal-truncated": "1" })).status).toBe(204);
		const saved = JSON.stringify(await archive(s.machine.id));
		for (const value of patterns) expect(saved).not.toContain(value);
		expect(saved).toContain("last diagnostic");
		expect(await env.DB.prepare("SELECT truncated FROM server_logs WHERE server_id = ?1").bind(s.OVERMATCH_SERVER_ID).first()).toEqual({ truncated: 1 });
	});
	it.each(["journal", "end"] as const)("expected D1 failures cross real %s RPC as data; SQL bugs still reject", async (operation) => {
		const s = await server();
		const sink = operation === "journal" ? "server_logs" : "server_crashes";
		await failD1Once(V, `INSERT INTO ${sink}`);
		const report = () => operation === "journal" ? push(s) : call("POST", `/v1/servers/${s.OVERMATCH_SERVER_ID}/end`, { crash: "panic fixture" }, { authorization: `Bearer ${s.OVERMATCH_REPORT_SECRET}` });
		const response = await report();
		expect(response.status).toBe(503);
		expect(await response.json()).toEqual({ error: "service unavailable; retry later" });
		await env.DB.prepare(`ALTER TABLE ${sink} RENAME TO fixture_evidence_unavailable`).run();
		try { await expect(report()).rejects.toThrow("no such table"); }
		finally { await env.DB.prepare(`ALTER TABLE fixture_evidence_unavailable RENAME TO ${sink}`).run(); }
	});

	it("missing pushes leave a visible NULL archive after the fixed window even after eviction", async () => {
		const s = await server();
		await call("POST", `/v1/servers/${s.OVERMATCH_SERVER_ID}/end`, {}, { authorization: `Bearer ${s.OVERMATCH_REPORT_SECRET}` });
		await age(V, provider().logs.exitGraceMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(s.machine.state).toBe("terminated");
		expect(await archive(s.machine.id)).toBeNull();
		await abortAllDurableObjects();
		await age(V, JOURNAL_WINDOW_MS + 1);
		await runDurableObjectAlarm(registry(V));
		expect(await archive(s.machine.id)).toMatchObject({ log: null, error: "journal push window closed without an archive" });
		expect((await push(s)).status).toBe(410);
	});

	it("an archive-close D1 outage keeps the durable check pending until storage recovers", async () => {
		const s = await server();
		await call("POST", `/v1/servers/${s.OVERMATCH_SERVER_ID}/end`, {}, { authorization: `Bearer ${s.OVERMATCH_REPORT_SECRET}` });
		await age(V, JOURNAL_WINDOW_MS + 1);
		await failD1Once(V, "journal push window closed");
		await runDurableObjectAlarm(registry(V));
		expect(await archive(s.machine.id)).toBeNull();
		expect(s.machine.state).toBe("terminated"); // Evidence failure cannot retain a machine/slot.
		await abortAllDurableObjects();
		await age(V, cfg.statusPollMs + 1);
		await runDurableObjectAlarm(registry(V));
		expect(await archive(s.machine.id)).toMatchObject({ log: null, error: "journal push window closed without an archive" });
	});

	it.each([undefined, null, "true"])("boot reports without a boolean unit fact (%s) are refused before changing state", async (journalAvailable) => {
		const s = await server();
		const response = await call("POST", `/v1/servers/${s.OVERMATCH_SERVER_ID}/end`, { reason: "boot-start", journalAvailable }, { authorization: `Bearer ${s.OVERMATCH_REPORT_SECRET}` });
		expect(response.status).toBe(400);
		expect(await response.json()).toEqual({ error: "journalAvailable: expected boolean" });
		expect(s.machine.state).toBe("running");
		expect((await registry(V).snapshot())[0].state).toBe("starting");
	});

	it.each([false, true])("boot unit creation=%s is the fact controlling termination grace", async (journalAvailable) => {
		const s = await server();
		const response = await call("POST", `/v1/servers/${s.OVERMATCH_SERVER_ID}/end`, { reason: "boot-start", journalAvailable }, { authorization: `Bearer ${s.OVERMATCH_REPORT_SECRET}` });
		expect(response.status).toBe(204);
		expect(s.machine.state).toBe(journalAvailable ? "running" : "terminated");
	});

	it.each([false, true])("repeated end with crash=%s cannot move the original durable stop time", async (crash) => {
		const s = await server();
		const path = `/v1/servers/${s.OVERMATCH_SERVER_ID}/end`;
		const headers = { authorization: `Bearer ${s.OVERMATCH_REPORT_SECRET}` };
		expect((await call("POST", path, {}, headers)).status).toBe(204);
		await age(V, provider().logs.exitGraceMs / 2);
		const stopAt = () => runInDurableObject(registry(V), (_, state) => state.storage.sql.exec<{ stop_at: number }>("SELECT stop_at FROM servers WHERE id = ?", s.OVERMATCH_SERVER_ID).one().stop_at);
		const original = await stopAt();
		expect((await call("POST", path, crash ? { crash: "late panic fixture" } : {}, headers)).status).toBe(crash ? 204 : 410);
		expect(await stopAt()).toBe(original);
		await age(V, provider().logs.exitGraceMs / 2 + 1);
		await runDurableObjectAlarm(registry(V));
		expect(s.machine.state).toBe("terminated");
	});

	it("invalid UTF-8 and missing metadata are typed refusals rather than corrupt operator archives", async () => {
		const s = await server();
		expect((await push(s, new Uint8Array([255]))).status).toBe(400);
		expect((await push(s, "text", { "x-journal-truncated": "invalid" })).status).toBe(400);
		expect(await archive(s.machine.id)).toBeNull();
	});
});
