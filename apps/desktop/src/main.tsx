import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./index.css";
import { ErrorBoundary, ErrorScreen } from "./components/ErrorBoundary";

const root = createRoot(document.getElementById("root") ?? document.body);

// App (and the store inside it) loads the host state before its first render.
import("./App").then(
  ({ App }) =>
    root.render(
      <StrictMode>
        <ErrorBoundary>
          <App />
        </ErrorBoundary>
      </StrictMode>,
    ),
  (error: unknown) => root.render(<ErrorScreen title="OmniPanel couldn't start" error={error} />),
);
// path filter test
