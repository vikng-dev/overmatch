import { host } from "./host.generated";
import type { Env } from "./config";
import { d1Call, platformCall, ProviderError } from "./provider";
import type { Provider } from "./provider";
import { diag } from "./reasons";

// Resolution: cap each archive at 1 MiB to bound D1 storage; retain the newest diagnostic lines.
// Bound bytes after redaction, not UTF-16 characters.
export const SERVER_LOG_MAX_BYTES = host.journal.maxBytes;

/** Archive redaction and its linear-time audit: .agents/scratch/aws-journal-slice-4.md. */
export function sanitizedTail(raw: string, providerToken: string): { log: string; truncated: boolean } {
	let clean = providerToken ? raw.replaceAll(providerToken, "[credential]") : raw;
	// Multiline values and split Debug arrays are unmasked: src/net and vendored netcode emit
	// neither; live report/netcode secrets are caught by their hex/array shapes.
	clean = clean
		.replace(/(\bOVERMATCH_[A-Z_]+[ \t]*[:=])[^\n]*/gi, "$1[credential]")
		.replace(/((?<![A-Za-z0-9])(?:ticket|secret|tokens?|password|authorization|private[_ -]?key|netcode[_ -]?key|env_vars|environment)\b["']?[ \t]*[:=][ \t]*(?:(?:Bearer|Basic)[ \t]+)?)(?:\[[^\[\]\n]*\]|\{[^{}\n]*\}|"(?:\\[^\n]|[^"\\\n])*"|'(?:\\[^\n]|[^'\\\n])*'|[^\s,;]+)/gi, "$1[credential]")
		.replace(/https?:\/\/\S+/gi, "[url]")
		.replace(/(?<![0-9a-f])(?:0x)?[0-9a-f]{64,}(?![0-9a-f])/gi, "[credential]")
		.replace(/(?<![A-Za-z0-9_-])[A-Za-z0-9_-]+\.[0-9a-f]{32}\b/gi, "[ticket]")
		// NetcodeConfig::Debug can expose its [u8; 32] key; base64 is not emitted by the server.
		// Own-credential encodings are removed separately at the authenticated RPC edge.
		.replace(/\[(?:[ \t]*[0-9]{1,3}[ \t]*,){31}[ \t]*[0-9]{1,3}[ \t]*\]/g, "[credential]")
		// Consume maximal tokens once, including a peer: prefix; never retry within an IPv6 run.
		.replace(/[a-z0-9_:.%]+/gi, (token) => {
			const colon = token.indexOf(":");
			if (colon < 0) return token;
			const prefix = colon > 0 && /[g-z_]/i.test(token.slice(0, colon)) ? token.slice(0, colon + 1) : "";
			const candidate = token.slice(prefix.length);
			return /^[0-9a-f:.]+(?:%[a-z0-9]+)?$/i.test(candidate) && /[0-9a-f]/i.test(candidate)
				&& (candidate.includes("::") || (candidate.match(/:/g)?.length ?? 0) >= 3) ? prefix + "[ip]" : token;
		})
		.replace(/(?:[0-9]{1,3}\.){3}[0-9]{1,3}/g, "[ip]");
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

/** The first archive wins; an admitted push can replace the deadline marker after its D1 write races. */
export async function recordJournal(env: Env, serverId: string, deploymentId: string, raw: string, truncated: boolean, timeoutMs: number): Promise<boolean> {
	const tail = sanitizedTail(raw, env.EDGEGAP_TOKEN);
	const saved = await d1Call("journalStorage", timeoutMs, () => env.DB.prepare(
		`INSERT INTO server_logs (server_id, deployment_id, captured_at, log, truncated, error)
		 VALUES (?1, ?2, ?3, ?4, ?5, NULL) ON CONFLICT (server_id) DO UPDATE SET
		 deployment_id = excluded.deployment_id, captured_at = excluded.captured_at, log = excluded.log,
		 truncated = excluded.truncated, error = NULL WHERE server_logs.log IS NULL`,
	).bind(serverId, deploymentId, Date.now(), tail.log, Number(truncated || tail.truncated)).run());
	return saved.meta.changes > 0;
}

export type EvidenceResult<T> = { ok: true; value: T } | { ok: false; cause: string };

/** Expected outside failures cross DO RPC as data; invariant exceptions still reject. */
export async function evidenceResult<T>(serverId: string, call: () => Promise<T>): Promise<EvidenceResult<T>> {
	try { return { ok: true, value: await call() }; }
	catch (err) {
		if (!(err instanceof ProviderError)) throw err;
		console.warn("evidence unavailable", serverId, diag(err));
		return { ok: false, cause: err.op };
	}
}

export async function evidenceCall<T>(op: string, call: () => Promise<EvidenceResult<T>>): Promise<T> {
	const result = await platformCall(op, call);
	if (!result.ok) throw new ProviderError(`${op}:${result.cause}`, 0, "unknown");
	return result.value;
}

/** Authenticated report secrets are generated hex, but old server credentials may be opaque. */
export function redactOwnCredential(raw: string, secret: string): { text: string; replaced: boolean } {
	const forms = [secret, btoa(secret)];
	if (/^[0-9a-f]{64}$/i.test(secret)) {
		const hex = [secret.toLowerCase(), secret.toUpperCase()];
		forms.unshift(...hex.flatMap((s) => ["0x" + s, "0X" + s]));
		forms.push(...hex, ...hex.map((s) => btoa(s)), btoa(String.fromCharCode(...secret.match(/../g)!.map((b) => parseInt(b, 16)))));
	}
	let text = raw;
	for (const form of forms) text = text.replaceAll(form, "[credential]");
	return { text, replaced: text !== raw };
}

/** Finalise an expired push window without overwriting a journal that arrived before it closed. */
export async function recordMissingJournal(env: Env, serverId: string, deploymentId: string, timeoutMs: number): Promise<void> {
	await d1Call("journalStorage", timeoutMs, () => env.DB.prepare(
		`INSERT INTO server_logs (server_id, deployment_id, captured_at, log, truncated, error)
		 VALUES (?1, ?2, ?3, NULL, 0, 'journal push window closed without an archive') ON CONFLICT (server_id) DO NOTHING`,
	).bind(serverId, deploymentId, Date.now()).run());
}
