import { describe, expect, it } from "vitest";
import { settleAll } from "../src/deadline";
import { outsideRpc } from "../src/provider";

describe("alarm and sweep task failures", () => {
	it("finishes independent cleanup and reports every rejection so the caller cannot hide an invariant", async () => {
		const failures = [new Error("fixture invariant one"), new Error("fixture invariant two")];
		let completed = false;
		const batch = settleAll([
			async () => { throw failures[0]; },
			async () => { completed = true; },
			async () => { throw failures[1]; },
		], 2);
		await expect(batch).rejects.toMatchObject({ errors: expect.arrayContaining(failures) });
		expect(completed).toBe(true);
	});

	it("classifies retryable platform failures but propagates remote invariants", async () => {
		await expect(outsideRpc("fixture", 100, async () => { throw Object.assign(new Error(), { retryable: true }); }))
			.rejects.toMatchObject({ op: "fixture:transport", outcome: "unknown" });
		const invariant = new Error("fixture invariant");
		await expect(outsideRpc("fixture", 100, async () => { throw invariant; })).rejects.toBe(invariant);
	});
});
