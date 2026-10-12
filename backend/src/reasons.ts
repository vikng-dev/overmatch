// The only failure texts that leave the Worker: ticket `reason`, D1 `sessions.reason`, /admin.
// Provider response bodies, status details and exception messages never become a reason — they may
// echo env values (netcode key, report secret) or URLs. Diagnostics log `diag()` output only.

export const REASONS = {
	noCapacity: "no capacity in the selected location",
	startRefused: "the provider refused to start the server",
	startFailed: "the server failed to start",
	startTimeout: "the server did not become ready in time",
	providerUnavailable: "the provider did not answer",
	noLocation: "no selected location could start a server",
	serverLost: "the server stopped reporting",
	serverEnded: "the server ended",
	providerTerminated: "the provider stopped the server",
} as const;

export type Reason = (typeof REASONS)[keyof typeof REASONS];

/** A server's own end reason, reduced to a short plain token for history. */
export function serverReason(raw: string): string {
	const clean = raw.replace(/[^A-Za-z0-9 ._-]/g, "").trim().slice(0, 64);
	return `server: ${clean || "exit"}`;
}

/** What may be logged about a failure: its type and, for a provider error, operation and HTTP
 *  status. Never a message or body. */
export function diag(err: unknown): string {
	if (err && typeof err === "object" && "op" in err && "status" in err) {
		return `${(err as { op: string }).op} HTTP ${(err as { status: number }).status}`;
	}
	return err instanceof Error ? err.name : typeof err;
}
