// The orphan sweep. Every minute (`triggers.crons` in wrangler.jsonc, `scheduled` in index.ts) the
// Worker lists every Edgegap deployment and stops each one tagged `om-<hex>` (`tagOf` in
// registry.ts) that no live claim owns. A failed stop needs no bookkeeping: the deployment is still
// listed a minute later and judged again, until Edgegap no longer lists it.
//
// Ownership is decided by the Registry alone, never by D1 history. A tag carries the random half of
// a server id; the deployment's status names its app version, which is the Registry's name and the
// id's other half (`idOf`). The Registry writes a claim's row before its deploy call, so a
// deployment of a valid version that no live row of that Registry owns (`liveDeployments`) is
// stopped at once. Decisions are per deployment: with several `om-` tags, one live claim keeps it.
// A deployment whose status names no valid version cannot be routed and is never stopped here: it is
// logged and counted `unattributed`, and left to Edgegap's max_duration. A status read or a Registry
// that does not answer stops nothing it would have decided.
//
// Bounds per sweep: SWEEP_MAX_PAGES listing pages (a hard bound far above the fleet; hitting it is
// logged), SWEEP_MAX_CHECKS status reads and SWEEP_MAX_STOPS stops, CONCURRENCY calls in flight,
// each bounded by PROVIDER_TIMEOUT_S. Checks and stops run in id order rotated by the minute
// (`rotated`), so candidates that keep failing cannot hold the budget from the rest.

import { config, type Env } from "./config";
import { bounded, settleAll } from "./deadline";
import { edgegapFor, type DeploymentStatus } from "./edgegap";
import { idOf, validVersion } from "./ids";
import { diag } from "./reasons";
import { registryFor } from "./registry";

export const SWEEP_MAX_PAGES = 100;
export const SWEEP_MAX_CHECKS = 50;
export const SWEEP_MAX_STOPS = 25;
/** The `triggers.crons` period: one rotation step per sweep. */
const SWEEP_PERIOD_MS = 60_000;
/** Below the platform's six simultaneous outbound connections per invocation. */
const CONCURRENCY = 5;
const TAG = /^om-([0-9a-f]{32})$/;

export interface SweepResult {
	at: number;
	/** Deployments carrying an `om-` tag. */
	listed: number;
	/** Tagged deployments whose status named no valid version: never stopped here. */
	unattributed: number;
	stopped: number;
	failed: number;
	/** The last failure, as diag() text. */
	error: string | null;
}

type Claim = { serverId: string; deploymentId: string };

export async function sweep(env: Env, now = Date.now()): Promise<SweepResult> {
	const cfg = config(env);
	const timeout = cfg.providerTimeoutMs;
	const provider = edgegapFor(env, timeout);
	const turn = Math.floor(now / SWEEP_PERIOD_MS);
	const result: SweepResult = { at: now, listed: 0, unattributed: 0, stopped: 0, failed: 0, error: null };
	const fail = (what: string, err: unknown) => {
		result.error = `${what}: ${diag(err)}`;
		console.error("sweep", result.error);
	};

	const tagged: { id: string; hexes: string[] }[] = [];
	try {
		const out = await provider.deployments(SWEEP_MAX_PAGES);
		if (out.truncated) console.warn("sweep: listing truncated at", SWEEP_MAX_PAGES, "pages");
		for (const d of out.items) {
			const hexes = d.tags.map((t) => TAG.exec(t)?.[1]).filter((h): h is string => !!h);
			if (hexes.length) tagged.push({ id: d.id, hexes });
		}
	} catch (err) {
		fail("list", err);
		await save(env, result, timeout);
		return result;
	}
	result.listed = tagged.length;

	const checks = rotated(tagged, turn);
	const stop: { id: string; why: string }[] = [];
	const claims = new Map<string, Claim[]>();
	await settleAll(
		checks.slice(0, SWEEP_MAX_CHECKS).map((d) => async () => {
			let st: DeploymentStatus;
			try {
				st = await provider.status(d.id);
			} catch (err) {
				fail("status", err);
				return;
			}
			if (st.phase === "gone") return;
			const version = st.version;
			if (!validVersion(version)) {
				result.unattributed++;
				console.warn("sweep: no version in status, left to max_duration", d.id);
				return;
			}
			claims.set(version, [...(claims.get(version) ?? []), ...d.hexes.map((hex) => ({ serverId: idOf(version, hex), deploymentId: d.id }))]);
		}),
		CONCURRENCY,
	);

	await settleAll(
		[...claims].map(([version, list]) => async () => {
			try {
				const live = new Set<string>(await bounded(timeout, async () => registryFor(env, version).liveDeployments(list)));
				for (const id of new Set(list.map((c) => c.deploymentId))) if (!live.has(id)) stop.push({ id, why: "no live claim" });
			} catch (err) {
				fail("registry", err);
			}
		}),
		CONCURRENCY,
	);

	if (stop.length > SWEEP_MAX_STOPS) console.warn("sweep:", stop.length - SWEEP_MAX_STOPS, "stops wait for a later sweep");
	await settleAll(
		rotated(stop, turn)
			.slice(0, SWEEP_MAX_STOPS)
			.map(({ id, why }) => async () => {
				try {
					await provider.stop(id);
					result.stopped++;
					console.warn("sweep stopped", id, why);
				} catch (err) {
					result.failed++;
					fail("stop", err);
				}
			}),
		CONCURRENCY,
	);

	await save(env, result, timeout);
	return result;
}

/** `items` in id order, starting at position `turn` mod their count: a budget of B covers each item
 *  in at least B of any `items.length` consecutive turns over the same set. */
export function rotated<T extends { id: string }>(items: T[], turn: number): T[] {
	const sorted = [...items].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
	const k = sorted.length ? turn % sorted.length : 0;
	return [...sorted.slice(k), ...sorted.slice(0, k)];
}

async function save(env: Env, r: SweepResult, timeout: number): Promise<void> {
	try {
		await bounded(timeout, () =>
			env.DB.prepare(
				`INSERT INTO sweep (id, at, listed, unattributed, stopped, failed, error) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
				 ON CONFLICT (id) DO UPDATE SET at = excluded.at, listed = excluded.listed, unattributed = excluded.unattributed,
				   stopped = excluded.stopped, failed = excluded.failed, error = excluded.error`,
			)
				.bind(r.at, r.listed, r.unattributed, r.stopped, r.failed, r.error)
				.run(),
		);
	} catch (err) {
		console.error("sweep state write failed", diag(err));
	}
}

/** The last sweep's result, for /admin; null before the first sweep or if D1 does not answer. */
export async function lastSweep(env: Env): Promise<SweepResult | null> {
	try {
		return await env.DB.prepare("SELECT at, listed, unattributed, stopped, failed, error FROM sweep WHERE id = 1").first<SweepResult>();
	} catch (err) {
		console.error("sweep state read failed", diag(err));
		return null;
	}
}
