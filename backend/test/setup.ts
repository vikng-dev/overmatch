import { abortAllDurableObjects, applyD1Migrations, listDurableObjectIds, reset, runInDurableObject } from "cloudflare:test";
import { afterEach, beforeAll, beforeEach, vi } from "vitest";
import { FakeEdgegap } from "./fake-edgegap";
import { env } from "./helpers";

declare global {
	// Each test gets a fresh fake, reachable from the test body.
	var edgegap: FakeEdgegap;
}

// One stub for the file, delegating to whichever fake is current: the stub is what Worker and
// Durable Object code calls as `fetch`, the fake behind it is per test.
beforeAll(() => {
	vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => globalThis.edgegap.fetch(input, init));
});

// Storage isolation is per file; every test here wants a clean registry and history.
beforeEach(async () => {
	await applyD1Migrations(env.DB, env.TEST_MIGRATIONS);
	await env.DB.batch(["sessions", "sweep", "server_logs", "server_crashes"].map((table) => env.DB.prepare(`DELETE FROM ${table}`)));
	globalThis.edgegap = new FakeEdgegap();
});

// Reset runtime state and wipe each registry so its constructor recreates the schema.
afterEach(async () => {
	async function clear(namespace: DurableObjectNamespace) {
		for (const id of await listDurableObjectIds(namespace)) {
			await runInDurableObject(namespace.get(id), async (_, state) => {
				await state.storage.deleteAlarm();
				await state.storage.deleteAll();
			});
		}
	}
	await clear(env.REGISTRY);
	await clear(env.FLEET);
	await abortAllDurableObjects();
	await reset();
});
