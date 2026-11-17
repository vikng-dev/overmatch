// One EC2 admission owner across version registries. The read/count/insert has no await;
// slots survive eviction and are released only after refusal, confirmed stop or swept absence.
import { isPlatformFault, ProviderError, type FleetResult, type Reservation } from "./provider";
import { diag } from "./reasons";
import { DurableObject } from "cloudflare:workers";
import { config, type Env } from "./config";
import { Ec2, MAX_HOSTS } from "./ec2";
import { tagOf, versionOf } from "./ids";

export class Fleet extends DurableObject<Env> {
	constructor(ctx: DurableObjectState, env: Env) {
		super(ctx, env);
		ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS slots (server_id TEXT PRIMARY KEY, created_at INTEGER NOT NULL)");
	}
	async reserve(serverId: string): Promise<FleetResult<boolean>> {
		if (!versionOf(serverId)) throw new Error("Fleet reserve requires a versioned server id before persistence");
		let out;
		try { out = await new Ec2(this.env, config(this.env).providerTimeoutMs).deployments(50); }
		catch (err) {
			if (!(err instanceof ProviderError)) throw err;
			console.error("Fleet inventory failed", diag(err));
			return { ok: false, cause: "provider" };
		}
		if (out.truncated) return { ok: true, value: false }; // Admission requires a complete inventory.
		const slots = this.reservations();
		if (!slots.ok) return slots;
		if (slots.value.some((s) => s.server_id === serverId)) return { ok: true, value: true };
		const claims = new Set(slots.value.map((s) => tagOf(s.server_id)));
		const unmanaged = out.items.filter((d) => !d.tags.some((tag) => claims.has(tag))).length;
		if (slots.value.length + unmanaged >= MAX_HOSTS) return { ok: true, value: false };
		return this.storage(() => {
			this.ctx.storage.sql.exec("INSERT INTO slots VALUES (?, ?)", serverId, Date.now());
			return true;
		});
	}

	release(serverId: string): FleetResult<void> {
		return this.storage(() => { this.ctx.storage.sql.exec("DELETE FROM slots WHERE server_id = ?", serverId); });
	}
	reservations(): FleetResult<Reservation[]> {
		return this.storage(() => this.ctx.storage.sql.exec<Reservation>("SELECT * FROM slots").toArray());
	}
	private storage<T>(call: () => T): FleetResult<T> {
		try { return { ok: true, value: call() }; }
		catch (err) {
			if (!isPlatformFault(err)) throw err;
			console.error("Fleet storage failed", diag(err));
			return { ok: false, cause: "storage" };
		}
	}
}
