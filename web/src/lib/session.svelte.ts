/**
 * Who the hub thinks we are, shared by every screen.
 *
 * The token itself is never here: it lives in a cookie the script cannot
 * read. What this holds is the answer to `GET /whoami`, which decides what
 * the masthead shows and which write controls are worth rendering. The hub
 * checks permissions again on every request, so hiding a control is a
 * courtesy, never the enforcement.
 */
import { whoami, type Whoami } from './api/client';

const anonymous: Whoami = { user: null, namespaces: [], scope: null };

class Session {
	/** The current identity; anonymous until `refresh` says otherwise. */
	who = $state<Whoami>(anonymous);
	/** True until the first `refresh` settles, so screens can wait instead
	 * of flashing a logged-out masthead. */
	loading = $state(true);

	/** Whether a token is in play. */
	get signedIn(): boolean {
		return this.who.user !== null;
	}

	/** Whether the caller may write in `ns`, as far as the UI can tell. */
	mayWrite(ns: string): boolean {
		if (!this.signedIn) return false;
		if (this.who.scope === 'read') return false;
		return this.who.namespaces.includes(ns);
	}

	/** Ask the hub again. */
	async refresh(): Promise<void> {
		this.who = await whoami();
		this.loading = false;
	}

	/** Forget the identity locally, after the hub cleared the cookie. */
	clear(): void {
		this.who = anonymous;
		this.loading = false;
	}
}

/** The one session, imported wherever identity matters. */
export const session = new Session();
