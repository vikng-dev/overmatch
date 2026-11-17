// One EC2 admission owner across version registries. The read/count/insert has no await;
// slots survive eviction and are released only after refusal, confirmed stop or swept absence.
import { DurableObject } from "cloudflare:workers";
import { config, type Env } from "./config";
import { Ec2, MAX_HOSTS } from "./ec2";
import { tagOf } from "./ids";

export class Fleet extends DurableObject<Env> {
	constructor(ctx: DurableObjectState, env: Env) {
		super(ctx, env);
		ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS slots (server_id TEXT PRIMARY KEY, created_at INTEGER NOT NULL)");
	}
	async reserve(serverId: string): Promise<boolean> {
		const out = await new Ec2(this.env, config(this.env).providerTimeoutMs).deployments(50);
		if (out.truncated) return false; // The complete fleet is needed before admitting a launch.
		const slots = this.reservations();
		if (slots.some((s) => s.server_id === serverId)) return true;
		const claims = new Set(slots.map((s) => tagOf(s.server_id)));
		const unmanaged = out.items.filter((d) => !d.tags.some((tag) => claims.has(tag))).length;
		if (slots.length + unmanaged >= MAX_HOSTS) return false;
		this.ctx.storage.sql.exec("INSERT INTO slots VALUES (?, ?)", serverId, Date.now());
		return true;
	}
	release(serverId: string): void { this.ctx.storage.sql.exec("DELETE FROM slots WHERE server_id = ?", serverId); }
	reservations(): { server_id: string; created_at: number }[] {
		return this.ctx.storage.sql.exec<{ server_id: string; created_at: number }>("SELECT * FROM slots").toArray();
	}
}
