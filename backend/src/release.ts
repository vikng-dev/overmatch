import { bounded } from "./deadline";
import { providerText, ProviderError } from "./provider";

// Resolution: one minute bounds GitHub reads and how long published or replaced assets remain stale, for hits and misses.
export const RELEASE_CACHE_MS = 60_000;

export interface ServerRelease { url: string; checksum: string }

/** GitHub's published release is the trust root for both download and checksum. */
export async function serverRelease(version: string, timeoutMs: number): Promise<ServerRelease> {
	const name = `overmatch-server-v${version}-x86_64-unknown-linux-gnu.tar.gz`;
	const url = `https://github.com/vikng-dev/overmatch/releases/download/v${encodeURIComponent(version)}/${encodeURIComponent(name)}`;
	try {
		return await bounded(timeoutMs, async (signal) => {
			const checksum = await fetch(`${url}.sha256`, { signal });
			if (!checksum.ok) {
				await checksum.body?.cancel();
				throw new ProviderError("release", checksum.status, checksum.status === 404 ? "releaseMissing" : "releaseUnavailable");
			}
			// Release checksums are one line, not unbounded provider bodies.
			const line = await providerText(checksum, 1024, "release", "releaseUnavailable");
			const match = /^([a-fA-F0-9]{64}) [ *]([^\r\n]+)\r?\n?$/.exec(line);
			if (!match || match[2] !== name) throw new ProviderError("release", 0, "releaseUnavailable");
			const archive = await fetch(url, { method: "HEAD", signal });
			await archive.body?.cancel();
			if (!archive.ok) throw new ProviderError("release", archive.status, archive.status === 404 ? "releaseMissing" : "releaseUnavailable");
			return { url, checksum: `${match[1].toLowerCase()}  ${name}` };
		});
	} catch (err) {
		if (err instanceof ProviderError) throw err;
		throw new ProviderError("release", 0, "releaseUnavailable");
	}
}
