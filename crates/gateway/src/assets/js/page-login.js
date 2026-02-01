// ── Login page ───────────────────────────────────────────────

import { html } from "htm/preact";
import { render } from "preact";
import { useState } from "preact/hooks";
import { registerPage } from "./router.js";

function LoginPage({ hasPasskeys }) {
	var [password, setPassword] = useState("");
	var [error, setError] = useState(null);
	var [loading, setLoading] = useState(false);

	function onPasswordLogin(e) {
		e.preventDefault();
		setError(null);
		setLoading(true);
		fetch("/api/auth/login", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ password }),
		})
			.then((r) => {
				if (r.ok) {
					location.href = "/";
				} else {
					setError("Invalid password");
					setLoading(false);
				}
			})
			.catch((err) => {
				setError(err.message);
				setLoading(false);
			});
	}

	function onPasskeyLogin() {
		setError(null);
		if (/^\d+\.\d+\.\d+\.\d+$/.test(location.hostname) || location.hostname.startsWith("[")) {
			setError(`Passkeys require a domain name. Use localhost instead of ${location.hostname}`);
			return;
		}
		setLoading(true);
		fetch("/api/auth/passkey/auth/begin", { method: "POST" })
			.then((r) => r.json())
			.then((data) => {
				var options = data.options;
				options.publicKey.challenge = base64ToBuffer(options.publicKey.challenge);
				if (options.publicKey.allowCredentials) {
					for (var c of options.publicKey.allowCredentials) {
						c.id = base64ToBuffer(c.id);
					}
				}
				return navigator.credentials
					.get({ publicKey: options.publicKey })
					.then((cred) => ({ cred, challengeId: data.challenge_id }));
			})
			.then(({ cred, challengeId }) => {
				var body = {
					challenge_id: challengeId,
					credential: {
						id: cred.id,
						rawId: bufferToBase64(cred.rawId),
						type: cred.type,
						response: {
							authenticatorData: bufferToBase64(cred.response.authenticatorData),
							clientDataJSON: bufferToBase64(cred.response.clientDataJSON),
							signature: bufferToBase64(cred.response.signature),
							userHandle: cred.response.userHandle ? bufferToBase64(cred.response.userHandle) : null,
						},
					},
				};
				return fetch("/api/auth/passkey/auth/finish", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify(body),
				});
			})
			.then((r) => {
				if (r.ok) {
					location.href = "/";
				} else {
					return r.text().then((t) => {
						setError(t || "Passkey authentication failed");
						setLoading(false);
					});
				}
			})
			.catch((err) => {
				setError(err.message || "Passkey authentication failed");
				setLoading(false);
			});
	}

	return html`<div class="auth-page">
		<div class="auth-card">
			<h1 class="auth-title">moltis</h1>
			<form onSubmit=${onPasswordLogin}>
				<div class="auth-field">
					<label class="settings-label">Password</label>
					<input
						type="password"
						class="settings-input"
						value=${password}
						onInput=${(e) => setPassword(e.target.value)}
						placeholder="Enter password"
						autofocus
					/>
				</div>
				${error ? html`<p class="auth-error">${error}</p>` : null}
				<button type="submit" class="settings-btn auth-submit" disabled=${loading}>
					${loading ? "Signing in\u2026" : "Sign in"}
				</button>
			</form>
			${
				hasPasskeys
					? html`<div class="auth-divider"><span>or</span></div>
				<button
					type="button"
					class="settings-btn auth-submit auth-passkey-btn"
					onClick=${onPasskeyLogin}
					disabled=${loading}
				>
					Sign in with passkey
				</button>`
					: null
			}
		</div>
	</div>`;
}

// ── Base64url helpers for WebAuthn ───────────────────────────

function base64ToBuffer(b64) {
	var str = b64.replace(/-/g, "+").replace(/_/g, "/");
	while (str.length % 4) str += "=";
	var bin = atob(str);
	var buf = new Uint8Array(bin.length);
	for (var i = 0; i < bin.length; i++) buf[i] = bin.charCodeAt(i);
	return buf.buffer;
}

function bufferToBase64(buf) {
	var bytes = new Uint8Array(buf);
	var str = "";
	for (var b of bytes) str += String.fromCharCode(b);
	return btoa(str).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

// ── Module state ─────────────────────────────────────────────

var containerRef = null;
var cachedHasPasskeys = false;

export function setHasPasskeys(v) {
	cachedHasPasskeys = v;
}

registerPage(
	"/login",
	(container) => {
		containerRef = container;
		container.style.cssText = "display:flex;align-items:center;justify-content:center;height:100%;";
		render(html`<${LoginPage} hasPasskeys=${cachedHasPasskeys} />`, container);
	},
	() => {
		if (containerRef) render(null, containerRef);
		containerRef = null;
	},
);
