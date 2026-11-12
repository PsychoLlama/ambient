# Authoring the Spec

Terse reference material for experts. Readers know the implementation, but not the context, properties, constraints, or future goals.

## Shape

- One page per topic.
- A page opens with a one or two sentence headline. Everything else is a section of bullets.
- Bullets are one or two sentences. No paragraphs, tables, or code blocks.
- Never repeat a fact or decision. It lives on one page; other pages link to it.
- Use the spec's own nouns consistently. A noun is introduced on the page that owns it and means the same thing everywhere.

## Scope

- Goals, decisions, properties, and constraints, never concrete implementations. Specs do not duplicate decisions that belong in code.
- Say only what is decided. If something is undecided, omit it.
- A `TODO:` bullet is for a problem that will bite, not for an idea. It sits on the page whose decisions depend on it.
- Don't document what an expert reader can infer.
- Less formal than a W3C specification, with the same posture: a reader could disagree with a sentence, not wonder what it meant.

## Conventions

- State a rule, then its consequence, as separate bullets. Give a reason only when the rule would otherwise look arbitrary.
- No text syntax is decided. Inline notation is illustrative, and a construct is described by what it does, not how it is written.
- Link a noun to its owning page the first time a page relies on it.
- A new page is added to the list in `README.md` with a one-line summary.
- Reversing a decision means editing the bullet that states it and every bullet that follows from it, not appending a correction.
- The spec leads the implementation; it never describes or references existing code.
