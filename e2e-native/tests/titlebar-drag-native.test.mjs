// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import assert from "node:assert/strict";
import test from "node:test";
import { By, until } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";

test("native titlebar drag regions move the window without consuming controls", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();

  try {
    if (!skipNativeBuild) ensureNativeAppBuilt(harness);
    assert.ok(harness.appPath);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  t.after(async () => {
    await session.close();
  });

  const driver = session.driver;
  await waitForAppShell(driver, 30_000);

  const dragRegions = await driver.findElements(By.css(
    '.titlebar-drag-spacer[data-tauri-drag-region]',
  ));
  assert.equal(dragRegions.length, 2);
  for (const region of dragRegions) {
    assert.equal(await region.isDisplayed(), true);
    assert.equal(await region.getCssValue("-webkit-app-region"), "drag");
  }

  const leftToggle = await driver.findElement(By.css('.titlebar-toggle[title="Show Left Sidebar"]'))
    .catch(() => driver.findElement(By.css('.titlebar-toggle[title="Hide Left Sidebar"]')));
  const rightToggle = await driver.findElement(By.css('.titlebar-right .titlebar-toggle'));
  await leftToggle.click();
  await driver.wait(until.elementLocated(By.css('.titlebar[data-left-collapsed="true"]')), 5000);
  await leftToggle.click();
  await driver.wait(until.elementLocated(By.css('.titlebar[data-left-collapsed="false"]')), 5000);
  await rightToggle.click();
  await driver.wait(until.elementLocated(By.css('.titlebar[data-right-collapsed="true"]')), 5000);
  await rightToggle.click();
  await driver.wait(until.elementLocated(By.css('.titlebar[data-right-collapsed="false"]')), 5000);

  const window = driver.manage().window();
  const before = await window.getRect();
  await window.setRect({ ...before, x: 180, y: 180 });
  const positioned = await window.getRect();
  const dragRegion = dragRegions[1];
  const regionRect = await dragRegion.getRect();
  assert.ok(regionRect.width > 1, "right empty chrome must expose a draggable area");

  await driver.actions({ async: true })
    .move({ origin: dragRegion, x: Math.max(1, Math.round(regionRect.width / 2)), y: Math.round(regionRect.height / 2) })
    .press()
    .move({ origin: dragRegion, x: Math.max(1, Math.round(regionRect.width / 2) + 80), y: Math.round(regionRect.height / 2), duration: 500 })
    .release()
    .perform();

  const after = await window.getRect();
  assert.ok(
    Math.abs(after.x - positioned.x) >= 20 || Math.abs(after.y - positioned.y) >= 20,
    `native window did not move after empty titlebar drag: before=${JSON.stringify(positioned)} after=${JSON.stringify(after)}`,
  );
});
