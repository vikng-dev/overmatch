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
	it("downstream consumers reuse the startup parse of numeric, URL, header and regex settings", async () => {
		const { config } = await import("../src/config");
		const { edgegapFor, Edgegap } = await import("../src/edgegap");
		const { providerName } = await import("../src/provider");
		const reads = new Map<string, number>();
		const settings = new Proxy({ ...env }, { get(target, key, receiver) {
			if (typeof key === "string") reads.set(key, (reads.get(key) ?? 0) + 1);
			return Reflect.get(target, key, receiver);
		} });
		const cfg = config(settings);
		const afterStartup = new Map(reads);
		expect(config(settings)).toBe(cfg);
		expect(providerName(settings)).toBe("edgegap");
		expect(edgegapFor(settings, cfg.providerTimeoutMs)).toBeInstanceOf(Edgegap);
		expect(reads).toEqual(afterStartup);
	});

	it.each(["JOIN_ATTEMPTS", "USD_PER_VCPU_MINUTE", "CAPACITY", "HEARTBEAT_INTERVAL_S", "HEARTBEAT_DEADLINE_S", "START_DEADLINE_S", "STATUS_POLL_S", "RESERVATION_TTL_S", "TICKET_TTL_S", "EMPTY_EXIT_S", "SERVER_STATS_S", "SERVER_LOG_RETENTION_DAYS", "ENDED_RETENTION_S", "LOCATIONS_CACHE_S", "PROVIDER_TIMEOUT_S"])("%s refuses malformed numbers with a named typed error", async (setting) => {
		const { config, ConfigurationError } = await import("../src/config");
		try {
			config({ ...env, [setting]: "fixture-invalid" });
			throw new Error("invalid operator number was accepted");
		} catch (err) {
			expect(err).toBeInstanceOf(ConfigurationError);
			expect(err).toMatchObject({ setting });
			expect((err as Error).message).not.toContain("fixture-invalid");
		}
	});
});
