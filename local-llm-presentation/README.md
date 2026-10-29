# The Next Word Is Ours

An interactive slide deck for the AI Council, proposing that we run our own private AI: open-weight language
models with our own harness and RAG. **Twenty slides, about twenty minutes, in four parts**:

1. **What an LLM is**: what it is and how large; how it learns; why a trained model is frozen and needs RAG; how
   it answers; temperature and top-K (a live instrument panel with a hallucination-risk gauge); memory, without and
   with a saved thread; forty terms; ten myths.
2. **Why our own pays off**: renting versus owning; what it costs (on-premises or AWS Mumbai); where it earns its keep.
3. **What we have built**: the harness rings, then one slide per ring: our documents (the RAG pipeline and
   ChromaDB ingestion), tools (a LangGraph supervisor routing to RAG, Infra and General agents), rules (human in the
   loop: a run that pauses at a checkpoint for approval) and the audit log (LangSmith traces showing the agent and
   the model chosen for every step); then the PC Companion prototype.
4. **How we would do it**: the four routes and our recommendation, and the pilot plan.

The deck says "our own" or "private" AI throughout, never "local" or "in-house", and names no Chinese models.
Every slide has its own entrance; the bottom bar shows the four parts, **G** (or clicking the slide counter) opens
a slide list, and the harness rings link to their slides. It is built for a PC screen.

Open `index.html` in a maximised browser window. Double-clicking works: no build step, no dependencies.
Fonts come from Google Fonts when online and fall back to system fonts offline.

- Every slide fits the window. One scroll (mouse wheel or trackpad), one arrow key, Page Up/Down or a swipe
  moves one slide. Home and End jump to the first and last slide.
- On slides with a technical deep-dive, an **Engineer's note** button appears in the bottom bar.
- Prices are in rupees with US dollars in brackets, converted at `INR_PER_USD` (top of `app.js`, currently
  ₹95.96 = $1). Calculator defaults are the `cSeats`, `cPrice`, `cHw`, `cRun` sliders in `index.html`;
  department examples, glossary, myths and routes are the `DEPTS`, `TERMS`, `MYTHS`, `PATHS` lists in `app.js`.
- The cost slide toggles between buying an on-premises server and renting an AWS GPU server in Mumbai
  (ap-south-1). AWS on-demand hourly prices, from AWS's published price list of 25 September 2026, are in
  `AWS_TYPES` in `app.js`; update them if AWS changes its prices.
