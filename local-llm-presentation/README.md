# The Next Word Is Ours

An interactive slide deck for the AI Council, proposing that we host open-weight language models in-house
with our own harness and RAG. Fourteen slides, about fifteen minutes, in four parts: what an LLM is (what it is
and how large, how it learns, how it answers, why a frozen model needs RAG, the vocabulary, ten myths), why our
own pays off (renting versus owning, what it costs, where it earns its keep), what we have built (the harness, the
PC Companion prototype), and how we would do it (the four routes and our recommendation, the pilot plan).

It is a condensed version of an earlier 28-slide deck that keeps nearly all of its information: reference
material (glossary, myths, department examples, routes) sits behind clicks, and the deck says "our own" or
"in-house" AI throughout, never "local".

Open `index.html` in a maximised browser window. Double-clicking works: no build step, no dependencies.
Fonts come from Google Fonts when online and fall back to system fonts offline.

- Every slide fits the window. One scroll (mouse wheel or trackpad), one arrow key, Page Up/Down or a swipe
  moves one slide. Home and End jump to the first and last slide.
- On slides with a technical deep-dive, an **Engineer's note** button appears in the bottom bar.
- On a phone or a very small window the slides stack and scroll normally.
- Prices are in rupees with US dollars in brackets, converted at `INR_PER_USD` (top of `app.js`, currently
  ₹95.96 = $1). Calculator defaults are the `cSeats`, `cPrice`, `cHw`, `cRun` sliders in `index.html`;
  department examples, glossary, myths and routes are the `DEPTS`, `TERMS`, `MYTHS`, `PATHS` lists in `app.js`.
- The cost slide toggles between buying an on-premises server and renting an AWS GPU server in Mumbai
  (ap-south-1). AWS on-demand hourly prices, from AWS's published price list of 25 September 2026, are in
  `AWS_TYPES` in `app.js`; update them if AWS changes its prices.
