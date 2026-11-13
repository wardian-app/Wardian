import { lazy, Suspense } from "react";
import ReactDOM from "react-dom/client";
import { ConfirmProvider } from "./components/ConfirmDialog";
import { RemoteMobileApp } from "./features/remote/RemoteMobileApp";
import { registerServiceWorker } from "./registerServiceWorker";
import { installChatPageObservation } from "./features/chat/chatPageObservation";

installChatPageObservation(window);

const search = new URLSearchParams(window.location.search);
const isRemoteShell = window.location.pathname.startsWith("/remote") || search.has("remote");
const DesktopApp = lazy(() => import("./views/App"));
const Root = isRemoteShell ? RemoteMobileApp : DesktopApp;

if (isRemoteShell) {
  registerServiceWorker();
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <ConfirmProvider>
    <Suspense fallback={null}>
      <Root />
    </Suspense>
  </ConfirmProvider>
);
