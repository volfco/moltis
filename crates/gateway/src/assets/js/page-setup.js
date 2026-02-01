// ── First-run setup page ─────────────────────────────────────

import { html } from "htm/preact";
import { render } from "preact";
import { useState } from "preact/hooks";
import { registerPage } from "./router.js";

function SetupPage() {
	var [password, setPassword] = useState("");
	var [confirm, setConfirm] = useState("");
	var [error, setError] = useState(null);
	var [saving, setSaving] = useState(false);

	function onSubmit(e) {
		e.preventDefault();
		setError(null);
		if (password.length < 8) {
			setError("Password must be at least 8 characters.");
			return;
		}
		if (password !== confirm) {
			setError("Passwords do not match.");
			return;
		}
		setSaving(true);
		fetch("/api/auth/setup", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ password }),
		})
			.then((r) => {
				if (r.ok) {
					location.href = "/";
				} else {
					return r.text().then((t) => {
						setError(t || "Setup failed");
						setSaving(false);
					});
				}
			})
			.catch((err) => {
				setError(err.message);
				setSaving(false);
			});
	}

	return html`<div class="auth-page">
		<div class="auth-card">
			<h1 class="auth-title">Welcome to moltis</h1>
			<p class="auth-subtitle">Set a password to secure your instance.</p>
			<form onSubmit=${onSubmit}>
				<div class="auth-field">
					<label class="settings-label">Password</label>
					<input
						type="password"
						class="settings-input"
						value=${password}
						onInput=${(e) => setPassword(e.target.value)}
						placeholder="At least 8 characters"
						autofocus
					/>
				</div>
				<div class="auth-field">
					<label class="settings-label">Confirm password</label>
					<input
						type="password"
						class="settings-input"
						value=${confirm}
						onInput=${(e) => setConfirm(e.target.value)}
						placeholder="Repeat password"
					/>
				</div>
				${error ? html`<p class="auth-error">${error}</p>` : null}
				<button type="submit" class="settings-btn auth-submit" disabled=${saving}>
					${saving ? "Setting up\u2026" : "Set password"}
				</button>
			</form>
		</div>
	</div>`;
}

var containerRef = null;

registerPage(
	"/setup",
	(container) => {
		containerRef = container;
		container.style.cssText = "display:flex;align-items:center;justify-content:center;height:100%;";
		render(html`<${SetupPage} />`, container);
	},
	() => {
		if (containerRef) render(null, containerRef);
		containerRef = null;
	},
);
