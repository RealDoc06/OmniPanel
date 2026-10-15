import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./index.css";
import { errorMessage } from "./lib/util";

const root = createRoot(document.getElementById("root") ?? document.body);

// App (and the store inside it) loads the host state before its first render.
import("./App").then(
  ({ App }) =>
    root.render(
      <StrictMode>
        <App />
      </StrictMode>,
    ),
  (error: unknown) =>
    root.render(
      <div className="flex h-full flex-col items-center justify-center gap-2 p-8 text-center">
        <p className="font-semibold">OmniPanel could not start</p>
        <p className="max-w-md text-muted" data-selectable>
          {errorMessage(error)}
        </p>
      </div>,
    ),
);
