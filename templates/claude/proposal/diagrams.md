# Diagrams for /proposal

## Contents
- Shared rules: file, classes, sizes, arrowhead
- Boxes: system context, container / component, deployment, network
- Sequence
- Entity-relationship (ER)
- Flowchart / activity
- Checklist

## Shared rules

**One file per diagram,** `diagrams/<kind>-<name>.svg`: one `<svg>` with
`xmlns="http://www.w3.org/2000/svg"`, a `viewBox`, `width` and `height` equal to the `viewBox`'s
size, `role="img"`, and a `<title>` as its first child saying what it shows in one sentence. No
`style` and no colours: the page styles it, in light and dark, and shrinks it when it doesn't fit.

**Classes,** all styled by `page.html`:
- shapes: `box` a part; add `new` for what the proposal adds or changes, `ext` for what is outside
  our and the readers' control; `group` a dashed boundary;
- lines: `line` a request or a data path; add `reply` for a reply, `dev` for a dev-only path, `new`
  for a line the proposal adds; `life` a sequence lifeline; `head` the arrowhead;
- text: `title` a box's name (13px), `sub` its second line (11px), `label` a line's label (11px),
  `note` a remark beside a lifeline (11px), `group-label` a boundary's name (10px, capitals); add
  `mid` to centre a text on its x, `end` to end it there;
- marks: `step` a numbered circle, with `check` for a check inside one system; `step-n` its number.

**Short:** a box's name at most 4 words and 22 characters, its second line at most 5 words; a line's
label at most 12 characters unless its recipe gives more room. A character is about 7px wide at
13px and 6px at 11px: a text must fit its room with 10px to spare. If it doesn't, shorten the text;
never make it smaller.

**Small:** at most 9 boxes and 12 lines. A bigger picture is two diagrams, or a cut.

**Straight lines only,** horizontal or vertical; a bend is a `path` with `H` and `V` segments.
Lines run through the gaps between boxes, never through a box or a label.

**Nothing overlaps,** and everything stays 16px inside the `viewBox`.

**The arrowhead,** the first thing after `<title>`. Its id carries the diagram's name, because two
diagrams pasted into one page must not share an id:

```svg
<defs><marker id="arrow-<name>" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path class="head" d="M 0 0 L 10 5 L 0 10 Z"/></marker></defs>
```

Every line that points uses `marker-end="url(#arrow-<name>)"`. The legend goes in the page, under
the diagram, and lists only the marks this diagram uses.

## Boxes: system context, container / component, deployment, network

One recipe, four meanings:

| Kind | Boxes are | Groups are | Lines are |
|---|---|---|---|
| System context | the system (one box) and the people and outside systems around it | one per organisation, or none | who uses or calls whom, labelled with what for |
| Container / component | the parts that run or store: apps, services, databases, queues, modules | the system's boundary | calls and data, labelled with the protocol or what moves |
| Deployment | what runs where: services, functions, stores | places, nested at most twice: account, then VPC or cluster; `dev` or `prod` in the label | only where it matters which place talks to which |
| Network | the gates and what sits behind them: load balancers, gateways, endpoints, firewalls | trust zones and subnets | allowed traffic, labelled with the port or protocol; `dev` for dev-only paths |

Put the readers' side on the left and ours on the right, so traffic reads left to right.

**Grid.** Column `c` at `x = 30 + 280·c`, row `r` at `y = 60 + 110·r`. A box is `160 × 56`,
`rx="8"`; its `title` at `(x + 12, y + 23)`, its `sub` at `(x + 12, y + 41)`. The gaps are 120px
across and 54px down. `viewBox` = `0 0 (280·columns − 60) (110·rows + 36)`. With a group inside
another, add 20 to every `x` and `y`, and 40 to the `viewBox` width and height.

**Lines.** Neighbours on one row: from `(x + 160, y + 28)` to `(x + 280, y + 28)`, the `label mid` at
`(x + 220, y + 22)`, at most 12 characters, 8 between two nested groups. Neighbours in one column: from `(x + 80, y + 56)` to
`(x + 80, y + 110)`, the `label` at `(x + 88, y + 87)`, at most 28 characters. Anything else: one
bend through the gaps, the label on its longest straight part. One line per pair of boxes: a reply
belongs in a sequence diagram.

**Groups.** A `group` rect 14px outside the boxes it holds on the sides and bottom, 28px above them.
With `left`, `top`, `right` and `bottom` the outer edges of those boxes: `x = left − 14`,
`y = top − 28`, `width = right − left + 28`, `height = bottom − top + 42`, `rx="10"`.
Its `group-label` at `(group x + 10, group y + 17)`, at most 24 characters. A group inside another:
the outer one 12px further out on every side and 22px higher.

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 500 146" width="500" height="146" role="img">
  <title>Portal users reach the new connect page over HTTPS</title>
  <defs><marker id="arrow-context" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path class="head" d="M 0 0 L 10 5 L 0 10 Z"/></marker></defs>
  <rect class="group" x="296" y="32" width="188" height="98" rx="10"/>
  <text class="group-label" x="306" y="49">OUR ACCOUNT</text>
  <rect class="box ext" x="30" y="60" width="160" height="56" rx="8"/>
  <text class="title" x="42" y="83">Portal users</text>
  <text class="sub" x="42" y="101">in a browser</text>
  <line class="line" x1="190" y1="88" x2="310" y2="88" marker-end="url(#arrow-context)"/>
  <text class="label mid" x="250" y="82">HTTPS</text>
  <rect class="box new" x="310" y="60" width="160" height="56" rx="8"/>
  <text class="title" x="322" y="83">Connect page</text>
  <text class="sub" x="322" y="101">one button</text>
</svg>
```

## Sequence

Who calls whom, in order, and what happens to the data on the way. At most 6 lifelines and 10
steps; the caller on the left.

**Lifelines.** Lifeline `i` at `x = 90 + 180·i`. Its header is a `box` `150 × 36` at
`(x − 75, 16)`, `rx="8"`, with its `title mid` at `(x, 39)`; then a `life` line from `(x, 52)` down
to `(x, height − 16)`.

**Steps.** Step `k` (from 1) on the row `y = 60 + 44·k`. A `step` circle, `r="9"`, on the caller's
lifeline at `(x, y)`, its `step-n` number at `(x, y + 4)`. The `line` runs from 9px past the circle
to the other lifeline, pointing; a reply is a `line reply`. Its `label mid` sits 6px above the line's
middle: at most 24 characters between neighbours, 30 more for each lifeline it crosses.

**Inside one system.** A check (valid? allowed?) or a change to the data (`Bronze → Silver`) has no
line: a `step check` circle on that lifeline and a `note` at `(x + 16, y + 4)`, at most 24
characters; on the last lifeline, a `note end` at `(x − 16, y + 4)`.

A step the proposal adds is a `line new`. `viewBox` = `0 0 (180·lifelines) (60 + 44·(steps + 1))`.

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 360 192" width="360" height="192" role="img">
  <title>The portal asks the API for a key, and the API checks it first</title>
  <defs><marker id="arrow-key" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path class="head" d="M 0 0 L 10 5 L 0 10 Z"/></marker></defs>
  <rect class="box" x="15" y="16" width="150" height="36" rx="8"/>
  <text class="title mid" x="90" y="39">Portal</text>
  <line class="life" x1="90" y1="52" x2="90" y2="176"/>
  <rect class="box" x="195" y="16" width="150" height="36" rx="8"/>
  <text class="title mid" x="270" y="39">API</text>
  <line class="life" x1="270" y1="52" x2="270" y2="176"/>
  <circle class="step" cx="90" cy="104" r="9"/><text class="step-n" x="90" y="108">1</text>
  <line class="line" x1="99" y1="104" x2="270" y2="104" marker-end="url(#arrow-key)"/>
  <text class="label mid" x="184" y="98">request + key</text>
  <circle class="step check" cx="270" cy="148" r="9"/><text class="step-n" x="270" y="152">2</text>
  <text class="note end" x="254" y="152">key valid? else 401</text>
</svg>
```

## Entity-relationship (ER)

Which records exist and how they relate. At most 7 entities, and only the fields the proposal
needs, at most 6 each.

**Entities.** Column `c` at `x = 30 + 320·c`; rows `70` apart below the tallest entity of the row
above, the first at `y = 30`. An entity is a `box` `200` wide and `38 + 18·fields` high, `rx="8"`:
its `title` at `(x + 12, y + 20)`, a `life` line across at `y + 30`, field `j` (from 0) as a `sub` at
`(x + 12, y + 43 + 18·j)`, and its key mark (`PK`, `FK`, or `new` for a field the proposal adds) as
a `sub end` at `(x + 188, same y)`. A new entity is a `box new`.

**Relations.** A `line` between facing sides, at the height of the field it joins, or one bend
through the gap; no arrowhead. The ends, at a side at `x = e`, height `m`, drawn as `line` paths:
- one: a bar across, 10px out: `M (e − 10) (m − 7) V (m + 7)` (mirror the offsets on a right side);
- many: a crow's foot: `M (e − 14) m L e (m − 7) M (e − 14) m L e (m + 7)`;
- zero or: add a `box` circle `r="4"` 22px out.
Its `label mid` is a verb of at most 12 characters (`places`), 6px above the middle, clear of the
ends. `viewBox` = `0 0 (320·columns − 60) (the lowest entity's bottom + 30)`.

## Flowchart / activity

Which path a case takes and where it branches. At most 12 nodes and 3 decisions, top to bottom.

**Columns.** The main path in the column centred at `x = 190`; a failure, retry or other branch in
the column at `x = 470`. Row `k` (from 0) centred at `y = 50 + 90·k`.

**Nodes,** each with its `title mid` at `(x, y + 5)`:
- start and end: a `box` `180 × 40`, `rx="20"`, at `(x − 90, y − 20)`;
- an action: a `box` `180 × 44`, `rx="8"`, at `(x − 90, y − 22)`, at most 22 characters;
- a decision: a `box` polygon `x,(y − 34) (x + 80),y x,(y + 34) (x − 80),y`, a question of at most
  14 characters ending in `?`.

**Lines.** Down the main column from a node's bottom to the next one's top, pointing. From a
decision, `yes` goes down, its `label` at `(x + 8, midpoint + 4)`; `no` goes right, from
`(x + 80, y)` to the side node's left edge, its `label mid` 6px above the middle. A way back runs up
the side column, then left into a main node's right side, and only where the side column is empty.

**Who does what.** When the actor matters, swap the two columns for lanes, at most 3: lane `l` is a
`group` at `(35 + 300·l, 20)`, `290 × (90·rows + 20)`, with its `group-label` at
`(45 + 300·l, 37)`; its nodes are centred at `x = 180 + 300·l`, row `k` at `y = 90 + 90·k`. A line
from lane to lane leaves a node's side and turns down in the gap below it.

`viewBox` = `0 0 600 (90·rows + 20)`, or `0 0 (300·lanes + 60) (90·rows + 60)` with lanes.

## Checklist

Before you start the next diagram, check this one:
- [ ] It answers the question written for it in `facts.md`, and nothing else.
- [ ] Within its limits: boxes, lines, steps, nodes, fields.
- [ ] Every text fits its room: characters × 7 (13px) or × 6 (11px), plus 10, at most the room.
- [ ] Nothing overlaps; every line ends on an edge or a lifeline; all inside the `viewBox`.
- [ ] Names match the outline word for word.
- [ ] What the proposal adds or changes is `new`, and nothing else is.
- [ ] Classes only, no colours; `width` and `height` match the `viewBox`; a `<title>` first; the
  arrowhead id carries the diagram's name.
