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
