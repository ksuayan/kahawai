# Editorial Style Guide

The house voice for the publication and every app built alongside it. One author, one standard, whether the reader is settling into a long essay or tapping a button.

This guide describes the tone and style established in the publication's long-form travel and art articles, then generalizes it into rules for application verbiage: interface labels, messages, help text, onboarding, notifications, and documentation. Part 1 is the source voice. Part 2 is how that voice behaves inside software. Part 3 is a quick reference for reviewers and AI agents working in this codebase.

The guide has two standards of equal weight:

1. **Every piece of writing should change something for the reader.** An article reorients how someone sees a subject. A line of interface copy moves someone forward with confidence.
2. **Every statement must be true.** Nothing is claimed that has not been verified. Writing that is engaging but inaccurate has failed. Writing that is accurate but useless has also failed.

---

## Part 1 — The Source Voice

### Who is speaking

We write as a knowledgeable, well-traveled insider: an intelligent friend who happens to know the subject deeply and is genuinely pleased to share it.

- Warm, confident, occasionally witty. Never flippant.
- Never a teacher addressing students, never a critic addressing a jury.
- Never condescending, never over-explaining, never hedging without reason.
- Treat the reader as an equal who will rise to meet the material.
- Humor comes from genuine connections and surprising detail, never from jokes inserted for their own sake.
- The emotional range runs from engaged and informative to genuinely moved. Neither end is suppressed.

### Who is listening

Educated, curious adults who read widely and travel seriously. They may be practitioners or admirers.

- Assume general cultural literacy. Do not define what the reader already knows.
- Do not assume specialist vocabulary. Introduce technical terms in context and let meaning emerge through use, rather than through parenthetical glosses.
- The reader should finish knowing more and seeing differently.

### How long-form pieces are built

- **Open concretely.** A scene, an object, a sensory detail, or a pointed observation. Never a thesis statement, a dictionary definition, or a rhetorical question.
- **Let the argument emerge** over the first few paragraphs before stating it outright.
- **Use specific, active section headers.** "The Medium That Made the Renaissance Possible," not "Historical Background."
- **Keep body prose continuous.** Sections flow as argument, not as lists of sub-points. Lists read as "inform and move on"; prose reads as "think with me."
- **Close by deepening, not summarizing.** The ending arrives at a new formulation of the idea or extends an invitation. The last line should be able to stand alone.
- **Reference material goes at the end.** Tables and quick-reference sections are welcome in practical pieces, after the prose.

### Sentence-level style

- Vary sentence length on purpose: short sentences for arrival and emphasis, longer ones to develop an idea.
- Each paragraph makes one move and earns the next.
- Explain through analogy, example, and consequence. Show the effect before naming the cause.
- Prefer active voice. Begin with the subject doing something, not with "This," "It," or "There."
- Use bold and italics sparingly: bold for real emphasis, italics for titles, foreign words, and occasional stress.
- Avoid hedging filler ("somewhat," "rather," "quite"), management-speak ("leverage," "optimize," "unpack"), and the false modesty of academic prose.

### Register by content type

| Content type | Register |
|---|---|
| History and art history | Richest texture; people treated as fully human, not monuments; technical detail serves the story |
| Technical materials | Precision first, then interest; specific, named recommendations; honest about trade-offs; no single "correct" method |
| Travel and shopping guides | A friend who has been there; honest about price, access, and effort; context makes the practical advice richer; admits what is variable |
| Reading lists and collections | Curatorial and opinionated; explains why each choice earns its place; honest about difficulty and cost |

### What we are not

- Not a beginner's primer.
- Not a product review or a sales pitch. Makers are discussed honestly, including limitations and where competitors serve better.
- Not an academic survey.
- Not aspirational lifestyle content. Craft and knowledge are genuine values, not status markers.
- Not a vehicle for plausible-sounding misinformation.

### The accuracy standard

This rule was codified after real errors were caught and corrected in our own published drafts. It is non-negotiable.

- **Never invent facts.** Names, dates, addresses, hours, prices, product details, and attributions are verified against a current source before they appear.
- **Plausible is not true.** A claim that fits the narrative is still a fabrication if it has not been checked.
- **When in doubt, leave it out.** A gap is better than a fabrication. If something cannot be verified, omit it, or flag it clearly as unconfirmed.
- **Distinguish kinds of claims.** A maker's own assertion, practitioner experience, and independently verified fact are different things and are labeled as such.
- **Name uncertainty honestly.** Where sources disagree, say so. Where details change seasonally, say so.
- **Never smooth over uncertainty for elegance.** A slightly less polished sentence that is accurate beats a beautiful one that is not.

**Error protocol.** When a mistake is found: acknowledge it plainly, identify exactly what was wrong, correct it from a verified source or remove it, and do not patch one unverified claim with another.

---

## Part 2 — Applying the Voice to Application Verbiage

Interface copy is not long-form writing. The reader is in the middle of a task, not settling in with an article. The *voice* carries over intact; the *form* compresses. Some long-form rules invert entirely: in an interface, short lists and scannable structure are often correct.

### What carries over directly

| Long-form principle | In the application |
|---|---|
| Knowledgeable friend, never condescending | Speak to users as capable adults. Do not over-explain, scold, or praise them for routine actions. |
| Explain through consequence | Tell users what will happen, not how the system works internally. |
| Specific, active headers | Specific, active labels: "Export report," not "Submit" or "OK." |
| Active voice, subject first | "We couldn't save your changes," not "Changes could not be saved." |
| No hedging, no management-speak | No "please note," "simply," "just," "leverage," "utilize," "seamlessly." |
| Honest about trade-offs | State the cost of an action before users commit to it. |
| Admit what is variable | Say when something may take a while, may change, or depends on conditions. |
| Accuracy is non-negotiable | The interface never states anything the system has not confirmed. |
| Error protocol | Error messages say what happened, why if known, and what to do next. |

### What changes

- **Length.** Lead with the point. Most interface text is a phrase or one to two sentences. If a message needs a paragraph, it probably belongs in help documentation.
- **Structure.** Lists, steps, and tables are appropriate when users scan or follow along. Prose is for explanation, not for instructions.
- **Openings.** The concrete-scene opening belongs to articles. In the interface, the first words carry the most important information.
- **Wit.** Allowed sparingly in low-stakes moments such as empty states or a completed milestone. Never in errors, warnings, destructive actions, security, billing, or anything involving personal, health, or financial data.
- **Emotional range.** Calm and steady by default. The interface does not celebrate trivial actions or dramatize failures.

### The accuracy standard in software

This is the most important translation. The interface is a narrator users trust completely, so it must never say more than it knows.

- **Confirm before claiming.** Show "Saved" only after the save succeeds. Show "Sent" only after delivery is confirmed; otherwise say "Sending…" or "Queued."
- **No invented precision.** Do not show "2 minutes remaining" unless the estimate is real. Prefer "This may take a few minutes."
- **Honest states.** Distinguish *failed*, *pending*, *partial*, and *unknown*. Never collapse "we don't know" into "success."
- **Name the source.** If data comes from an external system, a cache, or may be out of date, say so: "Last updated 9:14 AM."
- **No fabricated reassurance.** Do not promise "Your data is safe" or "This won't affect anything" unless that has been verified to be true for this action.
- **Generated or AI-assisted text** is labeled as such and is never presented as verified fact without a source.

### Patterns by context

**Buttons and actions**
- Verb plus object: "Save draft," "Delete 3 files," "Send invite."
- The label matches the result. A button labeled "Delete" deletes; it does not archive.
- Avoid "OK," "Yes," "Submit," and "Click here."

**Error messages**
- Structure: *what happened* → *why, if known* → *what to do*.
- Take responsibility when the system failed. Never blame the user for the system's limits.
- No codes without explanation; codes may follow the message for support purposes.
- Example: "We couldn't upload *report.pdf*. The file is larger than 25 MB. Try compressing it or splitting it into smaller files."

**Warnings and destructive actions**
- State the consequence plainly, including whether it can be undone.
- Name the specific thing affected.
- Example: "Delete *Q3 Planning*? This removes the project and its 14 documents for everyone. This can't be undone."
- Buttons: "Delete project" and "Cancel," never "Yes" and "No."

**Empty states**
- Say what belongs here and how to add it. One sentence of orientation and one clear action.
- A little warmth is welcome here; this is where the friendly voice can show.
- Example: "No saved searches yet. Save a search to find it here next time."

**Success and confirmations**
- Brief and factual. Confirm what happened; add a next step only if useful.
- No exclamation points for routine actions.
- Example: "Invite sent to alex@example.com."

**Onboarding and help text**
- Explain by consequence: what users can do and why it matters to them, not feature inventories.
- Introduce terms in context, as in the articles.
- Keep tooltips to one sentence.

**Notifications**
- Lead with what changed and who changed it. Make them actionable or leave them out.
- Example: "Priya commented on *Launch checklist*."

**Loading and progress**
- Describe the real operation: "Importing 240 records…" rather than a generic "Please wait."
- Say when something is taking longer than expected, and offer an alternative if one exists.

### Word choices

| Avoid | Prefer |
|---|---|
| Please note that… | *(state the fact)* |
| Simply / just / easily | *(omit)* |
| Utilize, leverage | Use |
| Oops! Something went wrong. | We couldn't load your files. Check your connection and try again. |
| Invalid input | Enter a date in MM/DD/YYYY format. |
| Are you sure? | Delete *Invoice 1042*? This can't be undone. |
| Success! | *(say what succeeded)* Changes saved. |
| Click here | *(describe the destination)* View billing history |
| Your data is secure. | *(only if verified; otherwise say what is actually done)* |
| Seamlessly, powerful, robust | *(show the benefit concretely, or omit)* |

### Mechanics

- Sentence case for labels, headers, and buttons.
- Contractions are fine and keep the tone human ("can't," "we'll").
- Address the user as "you." Use "we" for the product, and only when the product took the action.
- Numerals for counts, dates, and times ("3 files," "9:14 AM").
- Avoid ALL CAPS for emphasis and avoid stacked punctuation.
- Keep terminology consistent. One concept, one word, everywhere in the product.

---

## Part 3 — Quick Reference

### Review checklist

Before shipping any copy, confirm:

- [ ] Is every statement true and confirmed by the system at the moment it appears?
- [ ] Does it lead with what the user most needs?
- [ ] Is it the shortest version that is still complete and kind?
- [ ] Does each action label say exactly what will happen?
- [ ] Do errors explain what happened and what to do next?
- [ ] Are consequences and irreversibility stated before destructive actions?
- [ ] Is it free of hedging, filler, jargon, and management-speak?
- [ ] Would a knowledgeable, respectful friend say it this way?
- [ ] Is humor absent from errors, warnings, security, billing, and sensitive data?
- [ ] Is terminology consistent with the rest of the product?

### Guidance for AI agents in this codebase

- Follow this guide for all user-facing strings, documentation, and commit or release notes meant for users.
- Never generate factual claims (dates, figures, names, policies, capabilities) from training data alone. Verify against the codebase, configuration, or a cited source, or leave a clearly marked `TODO: verify` for a human.
- Never write copy that implies an outcome the code does not guarantee. Read the code path before writing its messages.
- When revising existing copy, preserve meaning and change only what the guide requires. Flag any statement you cannot verify rather than rewording around it.
- If a message would need a paragraph to be accurate, propose moving the detail to help documentation and linking to it.

### The two instructions that matter most

**Move the reader.** In an article, change how they see. In the interface, move them forward with confidence and without confusion.

**Tell the truth.** Trust is built statement by statement and lost with a single false one. If it cannot be verified, it does not ship.
