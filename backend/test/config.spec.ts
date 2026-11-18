import { afterEach, describe, expect, it, vi } from "vitest";
import productionServer from "../production-server.json";
import { env } from "./helpers";

afterEach(() => {
	vi.doUnmock("../production-server.json");
	vi.resetModules();
});

describe("production allocation admission", () => {
	// Fixture allocations probe the finite, positive database contract before a player can join.
	it.each([0, -1, NaN, Infinity, -Infinity, "2", null, undefined])("refuses vcpu=%s at module load", async (vcpu) => {
		vi.doMock("../production-server.json", () => ({ default: { vcpu } }));
		vi.resetModules();
		await expect(import("../src/config")).rejects.toThrow(
			`config: production-server.json vcpu=${String(vcpu)} (need a finite number > 0)`,
		);
	});

	it.each([0.25, 1.5, productionServer.vcpu])("retains the admitted vcpu=%s for session records", async (vcpu) => {
		vi.doMock("../production-server.json", () => ({ default: { vcpu } }));
		vi.resetModules();
		const { config } = await import("../src/config");
		expect(config(env).vcpuPerServer).toBe(vcpu);
	});
});

describe("operator settings admission", () => {
	it("launch settings refuse the server's out-of-domain values before any launch", async () => {
		const { config, ConfigurationError, SERVER_SETTING_DOMAINS, serverEnvironment } = await import("../src/config");
		for (const [setting, domain] of Object.entries(SERVER_SETTING_DOMAINS)) {
			const minimum = "exclusiveMin" in domain ? 0.25 : domain.min;
			const maximum = "exclusiveMax" in domain ? domain.max * (1 - Number.EPSILON) : domain.max;
			for (const value of [minimum, maximum]) {
				const checked = config({ ...env, [setting]: String(value) });
				const forwarded = serverEnvironment(checked, "fixture", "https://worker.test", "fixture-key", "fixture-report");
				expect(Number(forwarded.find((entry) => entry.key === `OVERMATCH_${setting}`)?.value)).toBe(value);
			}
			const invalid = [domain.min - 1, domain.max * 2];
			if ("integer" in domain) invalid.push(domain.min + 0.5);
			if ("exclusiveMin" in domain) invalid.push(domain.min);
			if ("exclusiveMax" in domain) invalid.push(domain.max);
			for (const value of invalid) {
				expect(() => config({ ...env, [setting]: String(value) })).toThrow(expect.objectContaining({ constructor: ConfigurationError, setting }));
			}
		}
	});

	it.each(["ACCESS_TEAM_DOMAIN", "ACCESS_AUD", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_HOST_CONFIG"] as const)("only absent %s disables its optional capability", async (setting) => {
		const { config, ConfigurationError } = await import("../src/config");
		const settings = { ...env, PROVIDER: "ec2", WORKER_ENV: "fixture", AWS_ACCESS_KEY_ID: undefined, AWS_SECRET_ACCESS_KEY: undefined, AWS_HOST_CONFIG: undefined };
		expect(() => config({ ...settings, [setting]: undefined })).not.toThrow();
		for (const value of ["", " ", "\t", "\ud800", null, 0]) {
			expect(() => config({ ...settings, [setting]: value })).toThrow(expect.objectContaining({ constructor: ConfigurationError, setting }));
		}
	});

	it.each(["///", "https://", "https://team.test/path", "https://team.test?query", "https://team.test#fragment", "https://team.test\n"])("supplied Access domain %s is checked even without an audience", async (value) => {
		const { config } = await import("../src/config");
		expect(() => config({ ...env, ACCESS_AUD: undefined, ACCESS_TEAM_DOMAIN: value })).toThrow(expect.objectContaining({ name: "ConfigurationError", setting: "ACCESS_TEAM_DOMAIN" }));
	});

	it("a valid Access origin is normalized only after admission", async () => {
		const { config } = await import("../src/config");
		expect(config({ ...env, ACCESS_TEAM_DOMAIN: "https://team.test///", ACCESS_AUD: "fixture-audience" }).access).toMatchObject({ issuer: "https://team.test", audience: "fixture-audience" });
	});

	it("downstream consumers reuse the startup parse of numeric, URL, header and regex settings", async () => {
		const { config } = await import("../src/config");
		const { edgegapFor, Edgegap } = await import("../src/edgegap");
		const reads = new Map<string, number>();
		const settings = new Proxy({ ...env }, { get(target, key, receiver) {
			if (typeof key === "string") reads.set(key, (reads.get(key) ?? 0) + 1);
			return Reflect.get(target, key, receiver);
		} });
		const cfg = config(settings);
		const afterStartup = new Map(reads);
		expect(config(settings)).toBe(cfg);
		expect(config(settings).provider.name).toBe("edgegap");
		expect(edgegapFor(settings, cfg.providerTimeoutMs)).toBeInstanceOf(Edgegap);
		expect(reads).toEqual(afterStartup);
	});

	it.each(["JOIN_ATTEMPTS", "USD_PER_VCPU_MINUTE", "CAPACITY", "HEARTBEAT_INTERVAL_S", "HEARTBEAT_DEADLINE_S", "START_DEADLINE_S", "STATUS_POLL_S", "RESERVATION_TTL_S", "TICKET_TTL_S", "EMPTY_EXIT_S", "SERVER_STATS_S", "SERVER_LOG_RETENTION_DAYS", "ENDED_RETENTION_S", "LOCATIONS_CACHE_S", "PROVIDER_TIMEOUT_S"])("%s refuses malformed numbers with a named typed error", async (setting) => {
		const { config, ConfigurationError } = await import("../src/config");
		for (const value of ["fixture-invalid", "", " ", "0x10", "Infinity", "1e1000"]) {
			expect(() => config({ ...env, [setting]: value })).toThrow(expect.objectContaining({ constructor: ConfigurationError, setting, message: expect.not.stringContaining("fixture-invalid") }));
		}
	});
	it.each(["HEARTBEAT_DEADLINE_S", "START_DEADLINE_S", "STATUS_POLL_S", "RESERVATION_TTL_S", "TICKET_TTL_S", "SERVER_LOG_RETENTION_DAYS", "ENDED_RETENTION_S", "LOCATIONS_CACHE_S", "PROVIDER_TIMEOUT_S"] as const)("%s refuses durations before an alarm or timer can overflow", async (setting) => {
		const { config, ConfigurationError } = await import("../src/config");
		expect(() => config({ ...env, [setting]: "1e100" })).toThrow(expect.objectContaining({ constructor: ConfigurationError, setting, message: expect.stringContaining("reduce this setting in wrangler vars") }));
	});

	it("the HTTP deadline fits setTimeout's signed 32-bit millisecond domain", async () => {
		const { config, ConfigurationError } = await import("../src/config");
		expect(config({ ...env, PROVIDER_TIMEOUT_S: String((2 ** 31 - 1) / 1000) }).providerTimeoutMs).toBe(2 ** 31 - 1);
		expect(() => config({ ...env, PROVIDER_TIMEOUT_S: String(2 ** 31 / 1000) })).toThrow(expect.objectContaining({ constructor: ConfigurationError, setting: "PROVIDER_TIMEOUT_S" }));
	});

});
