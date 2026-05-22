export function registerServiceWorker() {
  if (!("serviceWorker" in navigator)) return;

  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/remote-sw.js", { scope: "/remote" }).catch((error) => {
      console.warn("Remote service worker registration failed", error);
    });
  });
}
