import ReactDOM from "react-dom/client";
import App from "./views/App";
import { ConfirmProvider } from "./components/ConfirmDialog";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <ConfirmProvider>
    <App />
  </ConfirmProvider>
);
