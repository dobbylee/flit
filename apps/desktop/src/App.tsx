import { copy } from "./copy";

function App() {
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
            <span className="foundation__indicator" aria-hidden="true" />
            <h2 id="foundation-status" role="status">
              {copy["foundation.status"]}
            </h2>
          </div>
          <p>{copy["foundation.boundary"]}</p>
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
