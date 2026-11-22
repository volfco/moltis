const { expect, test } = require("../base-test");
const { navigateAndWait, waitForWsConnected, watchPageErrors } = require("../helpers");

// Regression coverage for the settings cold-load race: initial page data used
// to be fetched with sendRpc before the WebSocket handshake completed, so the
// requests failed silently (UNAVAILABLE / dropped / timed out) and the sections
// rendered defaults instead of the saved config without ever retrying.
//
// Deterministic: the first WebSocket is held unreachable until after the page
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
		const sentAfterRelease = [];
		await page.routeWebSocket(/\/ws\/chat$/, (ws) => {
			if (released) {
				// Post-release connection: transparent proxy that records
				// page→server requests so we can assert dispatch ordering.
				const server = ws.connectToServer();
				ws.onMessage((message) => {
					sentAfterRelease.push(String(message));
					server.send(message);
				});
				server.onMessage((message) => ws.send(message));
				return;
			}
			// First connection: the page-side socket opens but nothing is ever
			// answered or forwarded, simulating "not connected yet". It is left
			// open so no close/auth-redirect side effects fire.
		});

		await navigateAndWait(page, "/settings/tools");
		await expect(page.getByRole("heading", { name: "Tools", exact: true })).toBeVisible();

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

		// The load settles with a real response: no silent-failure symptoms.
		await expect(page.getByRole("button", { name: "Refresh", exact: true })).toBeEnabled({ timeout: 15_000 });
		await expect(
			page.getByText(/timed out|Service temporarily unavailable|WebSocket (not connected|disconnected)/),
		).toHaveCount(0);

		expect(pageErrors).toEqual([]);
	});
});
