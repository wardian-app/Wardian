import ReactDOM from "react-dom/client";
import App from "./views/App";
import { ConfirmProvider } from "./components/ConfirmDialog";
import { RemoteMobileApp } from "./features/remote/RemoteMobileApp";

const search = new URLSearchParams(window.location.search);
const isRemoteShell = window.location.pathname.startsWith("/remote") || search.has("remote");
const Root = isRemoteShell ? RemoteMobileApp : App;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <ConfirmProvider>
    <Root />
  </ConfirmProvider>
);
