const { expect, test } = require("../base-test");
const { navigateAndWait, watchPageErrors } = require("../helpers");

test.describe("Settings > Memory page", () => {
	test("memory settings page loads without errors", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/memory");

		await expect(page.getByRole("heading", { name: "Memory", exact: true })).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("agent self-improvement section is visible", async ({ page }) => {
		await navigateAndWait(page, "/settings/memory");

		await expect(page.getByText("Agent Self-Improvement")).toBeVisible();
	});

	test("skill self-improvement toggle is present and checked by default", async ({ page }) => {
		await navigateAndWait(page, "/settings/memory");

		const label = page.getByText("Skill self-improvement prompting");
		await expect(label).toBeVisible();
	});

	test("memory recall toggle is present", async ({ page }) => {
		await navigateAndWait(page, "/settings/memory");

		await expect(page.getByText("Memory recall (prefetch)")).toBeVisible();
	});

	test("periodic extraction toggle is present", async ({ page }) => {
		await navigateAndWait(page, "/settings/memory");

		await expect(page.getByText("Periodic memory extraction")).toBeVisible();
	});

	test("session-end summary toggle is present", async ({ page }) => {
		await navigateAndWait(page, "/settings/memory");

		await expect(page.getByText("Session-end summary")).toBeVisible();
	});

	test("save button is visible", async ({ page }) => {
		await navigateAndWait(page, "/settings/memory");

		await expect(page.getByRole("button", { name: "Save", exact: false })).toBeVisible();
	});

	test("postgres backend reveals the connection URL field", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/memory");

		// The PostgreSQL connection panel is hidden until the backend is selected.
		await expect(page.getByText("PostgreSQL connection")).toHaveCount(0);

		await page.getByRole("button", { name: "PostgreSQL", exact: true }).click();

		await expect(page.getByText("PostgreSQL connection")).toBeVisible();
		await expect(page.getByPlaceholder(/postgres:\/\//)).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("saving the postgres backend persists the choice", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/memory");

		await page.getByRole("button", { name: "PostgreSQL", exact: true }).click();
		await page.getByRole("button", { name: "Save", exact: true }).click();
		await expect(page.getByText("Saved", { exact: true })).toBeVisible();

		// Reload: the backend choice comes back from memory.config.get.
		await navigateAndWait(page, "/settings/memory");
		await expect(page.getByText("PostgreSQL connection")).toBeVisible();

		// Restore the default so later specs see a built-in backend.
		await page.getByRole("button", { name: "Built-in (Recommended)", exact: true }).click();
		await page.getByRole("button", { name: "Save", exact: true }).click();
		await expect(page.getByText("Saved", { exact: true })).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("unchanged built-in backend can be saved", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/memory");

		await page.getByRole("button", { name: "Save", exact: true }).click();

		await expect(page.getByText("Saved", { exact: true })).toBeVisible();
		expect(pageErrors).toEqual([]);
	});
});
