# The Next Word Is Ours

An interactive slide deck for the AI Council on LLMs and the systems around them: how language models work,
how AI helps us, and the harness we have built around a model. **Fifteen slides, about fifteen minutes, in three parts**:

1. **What an LLM is**: what it is and how large; how it learns; why a trained model is frozen and needs RAG; how
   it answers; temperature and top-K (a live instrument panel with a hallucination-risk gauge); memory, without and
   with a saved thread; forty terms; ten myths.
2. **The benefits of AI**: how AI helps us, department by department.
3. **What we have built**: the harness rings, then one slide per ring: our documents (the RAG pipeline and
   ChromaDB ingestion), tools (a LangGraph supervisor routing to RAG, Infra and General agents), rules (human in the
   loop: a run that pauses at a checkpoint for approval) and the audit log (LangSmith traces showing the agent and
   the model chosen for every step).

The deck never says "local" or "in-house", names no Chinese models and does not propose running models on laptops.
Every slide has its own entrance; the bottom bar shows the three parts, **G** (or clicking the slide counter) opens
a slide list, and the harness rings link to their slides. It is built for a PC screen.

Open `index.html` in a maximised browser window. Double-clicking works: no build step, no dependencies.
Fonts come from Google Fonts when online and fall back to system fonts offline.

- Every slide fits the window. One scroll (mouse wheel or trackpad), one arrow key, Page Up/Down or a swipe
  moves one slide. Home and End jump to the first and last slide.
- On slides with a technical deep-dive, an **Engineer's note** button appears in the bottom bar.
- Prices are in rupees with US dollars in brackets, converted at `INR_PER_USD` (top of `app.js`, currently
  ₹95.96 = $1). Department examples, glossary and myths are the `DEPTS`, `TERMS` and `MYTHS` lists in `app.js`.
