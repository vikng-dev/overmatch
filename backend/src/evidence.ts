import type { Env } from "./config";
import { d1Call, ProviderError } from "./provider";
import type { Provider } from "./provider";
import { diag } from "./reasons";

// Resolution: cap each archive at 1 MiB to bound D1 storage; retain the newest diagnostic lines.
// Bound bytes after redaction, not UTF-16 characters.
export const SERVER_LOG_MAX_BYTES = 1024 * 1024;

/** Hosting diagnostics omit configured secrets, but arbitrary panic messages reach the default
 * hook before Worker sanitisation. Netcode also logs peer addresses and token whitelist diagnostics.
 * Redact credential-bearing lines, URLs and crash_data env dumps; mask generated 32-byte hex
 * credentials even without a field label. This sanitises archives, not the container's raw output. */
function sanitizedTail(raw: string, providerToken: string): { log: string; truncated: boolean } {
	let clean = raw.split("\n").map((line) =>
		/ticket|OVERMATCH_|secret|token|password|authorization|private[_ -]?key|netcode[_ -]?key|env_vars|environment/i.test(line)
			? "[credential line redacted]" : line,
	).join("\n");
	if (providerToken) clean = clean.replaceAll(providerToken, "[credential]");
	clean = clean
		.replace(/https?:\/\/\S+/gi, "[url]")
		.replace(/\b[0-9a-f]{64,}\b/gi, "[credential]")
		.replace(/\b[A-Za-z0-9_-]+\.[0-9a-f]{32}\b/gi, "[ticket]")
		// Whole IPv6 tokens include mapped IPv4 and zone ids; preserve Rust paths and clock timestamps.
		.replace(/(?<![\w:])[0-9a-f]*:[0-9a-f:.]+(?:%[a-z0-9]+)?(?![\w:])/gi, (candidate) =>
			/[0-9a-f]/i.test(candidate) && (candidate.includes("::") || (candidate.match(/:/g)?.length ?? 0) >= 3) ? "[ip]" : candidate)
		.replace(/(?:\d{1,3}\.){3}\d{1,3}/g, "[ip]");
	const bytes = new TextEncoder().encode(clean);
	let start = Math.max(0, bytes.length - SERVER_LOG_MAX_BYTES);
	// A UTF-8 tail begins at a code point boundary; replacement glyphs could exceed the byte cap.
	while (start < bytes.length && (bytes[start] & 0xc0) === 0x80) start++;
	return { log: new TextDecoder().decode(bytes.subarray(start)), truncated: start > 0 };
}

/** Keep the first panic even if provider log capture fails or the deployment id is still unknown. */
export async function recordCrash(env: Env, serverId: string, crash: string, timeoutMs: number): Promise<void> {
	const tail = sanitizedTail(crash, env.EDGEGAP_TOKEN);
	await d1Call("evidenceStorage", timeoutMs, () => env.DB.prepare(
		`INSERT INTO server_crashes (server_id, captured_at, crash, truncated) VALUES (?1, ?2, ?3, ?4)
		 ON CONFLICT (server_id) DO NOTHING`,
	).bind(serverId, Date.now(), tail.log, Number(tail.truncated)).run());
}

/** The committed marker precedes all log reads. Any existing row skips capture, so an invocation
 * killed during capture cannot make its stop retries refetch the same body and die again.
 * Handled capture failures still stop in this invocation; an interrupted one needs a later stop. */
export async function stopWithEvidence(env: Env, provider: Provider, serverId: string, deploymentId: string, timeoutMs: number, absenceAfter?: number): Promise<void> {
	if (provider.logs.kind === "push") return provider.stop(deploymentId, absenceAfter);
	try {
		const mark = await d1Call("evidenceStorage", timeoutMs, () => env.DB.prepare(
			`INSERT INTO server_logs (server_id, deployment_id, captured_at, log, truncated, error)
			 VALUES (?1, ?2, ?3, NULL, 0, 'capture incomplete') ON CONFLICT (deployment_id) DO NOTHING`,
		).bind(serverId, deploymentId, Date.now()).run());
		if (mark.meta.changes > 0) {
			const tail = sanitizedTail(await provider.logs.read(deploymentId), env.EDGEGAP_TOKEN);
			await d1Call("evidenceStorage", timeoutMs, () => env.DB.prepare(
				"UPDATE server_logs SET log = ?2, truncated = ?3, error = NULL WHERE deployment_id = ?1",
			).bind(deploymentId, tail.log, Number(tail.truncated)).run());
		}
	} catch (err) {
		if (!(err instanceof ProviderError)) throw err;
		const error = diag(err);
		console.warn("server log capture failed; stopping anyway", serverId, deploymentId, error);
		try {
			await d1Call("evidenceStorage", timeoutMs, () => env.DB.prepare(
				`INSERT INTO server_logs (server_id, deployment_id, captured_at, log, truncated, error) VALUES (?1, ?2, ?3, NULL, 0, ?4)
				 ON CONFLICT (deployment_id) DO UPDATE SET captured_at = excluded.captured_at, error = excluded.error
				 WHERE server_logs.log IS NULL`,
			).bind(serverId, deploymentId, Date.now(), error).run());
		} catch (writeError) {
			if (!(writeError instanceof ProviderError)) throw writeError;
			console.warn("server log failure record failed", serverId, diag(writeError));
		}
	}
	await provider.stop(deploymentId, absenceAfter);
}

export async function expireServerLogs(env: Env, before: number, timeoutMs: number): Promise<void> {
	try {
		await d1Call("evidenceStorage", timeoutMs, () => env.DB.batch([
			env.DB.prepare("DELETE FROM server_logs WHERE captured_at < ?1").bind(before),
			env.DB.prepare("DELETE FROM server_crashes WHERE captured_at < ?1").bind(before),
		]));
	} catch (err) {
		if (!(err instanceof ProviderError)) throw err;
		console.warn("server log retention failed", diag(err));
	}
}

// Resolution: allow shutdown/network scheduling jitter without admitting old credentials indefinitely.
export const JOURNAL_WINDOW_MS = 120_000;

/** A first writer wins across retries and concurrent invocations; the machine never retries a push. */
export async function recordJournal(env: Env, serverId: string, deploymentId: string, raw: string, truncated: boolean, timeoutMs: number): Promise<boolean> {
	const tail = sanitizedTail(raw, env.EDGEGAP_TOKEN);
	const saved = await d1Call("journalStorage", timeoutMs, () => env.DB.prepare(
		`INSERT INTO server_logs (server_id, deployment_id, captured_at, log, truncated, error)
		 VALUES (?1, ?2, ?3, ?4, ?5, NULL) ON CONFLICT (server_id) DO NOTHING`,
	).bind(serverId, deploymentId, Date.now(), tail.log, Number(truncated || tail.truncated)).run());
	return saved.meta.changes > 0;
}
