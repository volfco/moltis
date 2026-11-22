const { expect, test } = require("../base-test");
const { navigateAndWait, waitForWsConnected, watchPageErrors } = require("../helpers");

// Regression coverage for the settings cold-load race: initial page data used
// to be fetched with sendRpc before the WebSocket handshake completed, so the
// requests failed silently (UNAVAILABLE / dropped / timed out) and the sections
// rendered defaults instead of the saved config without ever retrying.
//
// Deterministic: the boot WebSocket is held unreachable until after the page
// mounts, then a real connection is established via forceReconnect(). All waits
// are on observable state; no timers or sleeps.
test.describe("Settings cold-load", () => {
	test("tools overview loads after a page mount that races the WebSocket handshake", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.addInitScript(() => {
			// Keep the RPC/wait budget large so slow CI never races the hold.
			window.__moltisTestRpcTimeoutMs = 30_000;
		});

		let released = false;
		let firstSocketHeld = false;
		let heldSocketCount = 0;
		const sentAfterRelease = [];
		const serverFrames = [];
		await page.routeWebSocket(/\/ws\/chat$/, (ws) => {
			if (released) {
				// Post-release connection: transparent proxy that records
				// traffic so we can assert dispatch ordering and response data.
				const server = ws.connectToServer();
				ws.onMessage((message) => {
					sentAfterRelease.push(String(message));
					server.send(message);
				});
				server.onMessage((message) => {
					serverFrames.push(String(message));
					ws.send(message);
				});
				return;
			}
			// First connection: the page-side socket opens but nothing is ever
			// answered or forwarded, simulating "not connected yet". It is left
			// open so no close/auth-redirect side effects fire.
			heldSocketCount += 1;
			firstSocketHeld = true;
		});

		await navigateAndWait(page, "/settings/tools");
		await expect(page.getByRole("heading", { name: "Tools", exact: true })).toBeVisible();

		// The race is only exercised if the boot socket really took the hold
		// branch: routeWebSocket handler delivery is asynchronous, so without
		// this gate the boot socket could take the proxy branch (if `released`
		// flipped first) and PRE-FIX code would pass the spec vacuously.
		await expect.poll(() => firstSocketHeld).toBe(true);
		expect(heldSocketCount).toBe(1);

		// Release the hold and establish a real connection.
		released = true;
		await page.evaluate(() => {
			window.__moltis_modules["ws-connect"].forceReconnect();
		});
		await waitForWsConnected(page);

		// The initial load is dispatched only after the connection is ready.
		await expect
			.poll(() => sentAfterRelease.filter((m) => m.includes('"chat.context"')).length, { timeout: 15_000 })
			.toBeGreaterThan(0);

		// The load settles without any silent-failure symptom (the settle alone
		// is not enough: the Refresh button also enables on error paths).
		await expect(page.getByRole("button", { name: "Refresh", exact: true })).toBeEnabled({ timeout: 15_000 });
		await expect(
			page.getByText(/timed out|Service temporarily unavailable|WebSocket (not connected|disconnected)/),
		).toHaveCount(0);
		await expect(page.getByText("Failed to load tools overview.")).toHaveCount(0);

		// Positive assertion fed by the chat.context response: the rendered
		// tool count must match the response payload, not the unloaded default.
		const contextRequest = sentAfterRelease
			.map((message) => {
				try {
					return JSON.parse(message);
				} catch {
					return null;
				}
			})
			.find((frame) => frame && frame.method === "chat.context");
		expect(contextRequest).toBeTruthy();
		const contextResponse = serverFrames
			.map((message) => {
				try {
					return JSON.parse(message);
				} catch {
					return null;
				}
			})
			.find((frame) => frame && frame.type === "res" && frame.id === contextRequest.id);
		expect(contextResponse?.ok).toBe(true);
		const tools = Array.isArray(contextResponse?.payload?.tools) ? contextResponse.payload.tools : [];
		const countText = `${tools.length} registered tool${tools.length === 1 ? "" : "s"}`;
		await expect(page.getByText(countText, { exact: true })).toBeVisible();

		expect(pageErrors).toEqual([]);
	});
});
