import type { Env } from "./config";
import { bounded } from "./deadline";
import type { Provider } from "./edgegap";
import { diag } from "./reasons";

// Resolution: 1 MiB tail. DERIVED from the owner's 140 KB / 42 min log plus about 250 KB of
// 5 s stats, with headroom for longer matches. Bound bytes after redaction, not UTF-16 characters.
export const SERVER_LOG_MAX_BYTES = 1024 * 1024;

/** The deployed server never logs hosting secrets (hosting.rs); netcode does log peer addresses
 * and token whitelist diagnostics. Drop credential-bearing lines and URLs defensively, including
 * crash_data env dumps. Mask the generated 32-byte hex credentials even without a field label. */
function sanitizedTail(raw: string, providerToken: string): { log: string; truncated: boolean } {
	let clean = raw.split("\n").map((line) =>
		/secret|token|password|authorization|private[_ -]?key|netcode[_ -]?key|env_vars|environment/i.test(line)
			? "[credential line redacted]" : line,
	).join("\n");
	if (providerToken) clean = clean.replaceAll(providerToken, "[credential]");
	clean = clean
		.replace(/https?:\/\/\S+/gi, "[url]")
		.replace(/\b[0-9a-f]{64,}\b/gi, "[credential]")
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

/** All Worker stop paths archive first. Each read/write/fetch is bounded; capture failure never
 * vetoes the stop. A stored log survives stop retries, registry retention and missing history. */
export async function stopWithEvidence(env: Env, provider: Provider, serverId: string, deploymentId: string, timeoutMs: number): Promise<void> {
	try {
		const stored = await bounded(timeoutMs, () => env.DB.prepare("SELECT log IS NOT NULL AS stored FROM server_logs WHERE deployment_id = ?1")
			.bind(deploymentId).first<{ stored: number }>());
		if (!stored?.stored) {
			const tail = sanitizedTail(await provider.containerLogs(deploymentId), env.EDGEGAP_TOKEN);
			await bounded(timeoutMs, () => env.DB.prepare(
				`INSERT INTO server_logs (server_id, deployment_id, captured_at, log, truncated, error) VALUES (?1, ?2, ?3, ?4, ?5, NULL)
				 ON CONFLICT (deployment_id) DO UPDATE SET captured_at = excluded.captured_at, log = excluded.log,
				 truncated = excluded.truncated, error = NULL WHERE server_logs.log IS NULL`,
			).bind(serverId, deploymentId, Date.now(), tail.log, Number(tail.truncated)).run());
		}
	} catch (err) {
		const error = diag(err);
		console.warn("server log capture failed; stopping anyway", serverId, deploymentId, error);
		try {
			await bounded(timeoutMs, () => env.DB.prepare(
				`INSERT INTO server_logs (server_id, deployment_id, captured_at, log, truncated, error) VALUES (?1, ?2, ?3, NULL, 0, ?4)
				 ON CONFLICT (deployment_id) DO UPDATE SET captured_at = excluded.captured_at, error = excluded.error
				 WHERE server_logs.log IS NULL`,
			).bind(serverId, deploymentId, Date.now(), error).run());
		} catch (writeError) {
			console.warn("server log failure record failed", serverId, diag(writeError));
		}
	}
	await provider.stop(deploymentId);
}

export async function expireServerLogs(env: Env, before: number, timeoutMs: number): Promise<void> {
	try {
		await bounded(timeoutMs, () => env.DB.prepare("DELETE FROM server_logs WHERE captured_at < ?1").bind(before).run());
	} catch (err) {
		console.warn("server log retention failed", diag(err));
	}
}
