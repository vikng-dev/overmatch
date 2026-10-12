export class DeadlineError extends Error {
	constructor(ms: number) {
		super(`no answer within ${ms} ms`);
		this.name = "DeadlineError";
	}
}

/** Run `fn` with an abort signal and give up after `ms`, whether or not `fn` honours the signal:
 *  the race, not the abort, is the bound. */
export async function bounded<T>(ms: number, fn: (signal: AbortSignal) => Promise<T>): Promise<T> {
	const ac = new AbortController();
	let timer: ReturnType<typeof setTimeout> | null = null;
	const expiry = new Promise<never>((_, reject) => {
		timer = setTimeout(() => {
			ac.abort();
			reject(new DeadlineError(ms));
		}, ms);
	});
	try {
		return await Promise.race([fn(ac.signal), expiry]);
	} finally {
		if (timer !== null) clearTimeout(timer);
	}
}

/** Run `tasks` with at most `limit` in flight; each settles independently. */
export async function settleAll(tasks: (() => Promise<unknown>)[], limit: number): Promise<void> {
	let next = 0;
	const worker = async () => {
		while (next < tasks.length) {
			const task = tasks[next++];
			await task().catch(() => undefined);
		}
	};
	await Promise.all(Array.from({ length: Math.min(limit, tasks.length) }, worker));
}
