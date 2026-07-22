import { useEffect, useState } from "react";

import { copy } from "./copy";
import { loadSystemHealth } from "./systemHealth";

type FoundationState = "checking" | "ready" | "unavailable";

function App() {
  const [state, setState] = useState<FoundationState>("checking");

  useEffect(() => {
    let active = true;

    void loadSystemHealth()
      .then((health) => {
        if (!active) return;

        const foundationReady =
          health.core === "ready" &&
          health.storage === "not_configured" &&
          health.providers === "not_configured";
        setState(foundationReady ? "ready" : "unavailable");
      })
      .catch(() => {
        if (active) setState("unavailable");
      });

    return () => {
      active = false;
    };
  }, []);

  return (
    <main className="foundation" aria-labelledby="foundation-title">
      <div className="foundation__glow" aria-hidden="true" />

      <section className="foundation__panel">
        <header className="foundation__header">
          <div className="foundation__mark" aria-hidden="true">
            F
          </div>
          <p className="foundation__eyebrow">{copy["foundation.phase"]}</p>
          <h1 id="foundation-title">{copy["foundation.title"]}</h1>
          <p className="foundation__summary">{copy["foundation.summary"]}</p>
        </header>

        <div className="foundation__rule" aria-hidden="true" />

        <section className="foundation__status-card" aria-labelledby="foundation-status">
          <div className="foundation__status-line">
            <span
              className={`foundation__indicator foundation__indicator--${state}`}
              aria-hidden="true"
            />
            <h2 id="foundation-status" role="status">
              {copy[`foundation.status.${state}`]}
            </h2>
          </div>
          <p>{copy[`foundation.boundary.${state}`]}</p>
        </section>

        <footer className="foundation__footer">
          <span>{copy["foundation.local"]}</span>
          <span aria-hidden="true">·</span>
          <span>{copy["foundation.noControls"]}</span>
        </footer>
      </section>
    </main>
  );
}

export default App;
