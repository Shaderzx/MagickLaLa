import { test, expect } from 'e2e';
import { studioProbe } from '../support/studio-probe.ts';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

test('Potion rack: four or more bottles stand in rows on shelf planks fed by one glass pipe per row instead of a branch per bottle, crowded ingredient names stay whole, every bottle name keeps one line, no unowned kind caption floats under the shelves, pipes keep air above the tags below them, and each plank ends at its own bottles', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['potion-rack']);

  for (const [scene, rack] of Object.entries<any>(report)) {
    expect({ scene, cut: rack.cut }).toEqual({ scene, cut: [] });
    expect({ scene, rack: rack.rack }).toEqual({ scene, rack: true });
    expect({ scene, pipes: rack.pipes }).toEqual({ scene, pipes: rack.rows });
    expect(rack.rows, scene).toBeLessThanOrEqual(3);
    expect({ scene, wrapped: rack.wrapped }).toEqual({ scene, wrapped: [] });
    expect({ scene, kindShown: rack.kindShown }).toEqual({ scene, kindShown: false });
    expect({ scene, grazes: rack.grazes }).toEqual({ scene, grazes: [] });
    expect({ scene, planks: rack.overhangingPlanks }).toEqual({ scene, planks: [] });
  }
});
test('True records: Check keeps a good brew lit, an AI prompt may say {{customer}} literally with a note instead of a hex, a Send that hands off is still a bottle, only steps that ran wear step numbers after a brew, a pantry toss undoes as one step, an empty draft with variables saves and asks before you leave it, and a wire message does not follow you to the next potion', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-true-records-'));
  try {
    const report = await studioProbe(['true-records', join(dir, 'studio.sqlite')]);

    expect(report.checkKeepsBrew).toEqual({ brew: 'Succeeded', stale: false });
    expect(report.literalBraces).toEqual({ check: 'valid', noted: true });
    expect(report.replies).toEqual(['first', 'second']);
    expect(report.stepNumbers).toEqual({ duplicates: 0, untakenNumbered: [] });
    expect(report.tossUndo).toEqual({ added: true, removedByUndo: true, variables: 1 });
    expect(report.emptyDraft).toEqual({ saved: true, asked: true, kept: 2 });
    expect(report.wireMessageLeft).toBe(false);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('Tidy bottles: after a brew no step number covers a bottle name, no label on a bottle overlaps another, and a bottle shows its kind only when the whole kind fits', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['tidy-bottles']);

  for (const [scene, bottles] of Object.entries<any>(report)) {
    expect({ scene, clashes: bottles.clashes }).toEqual({ scene, clashes: [] });
    expect({ scene, cutKinds: bottles.cutKinds }).toEqual({ scene, cutKinds: [] });
  }
});
test('Starter shelf: an empty pot offers its glowing trigger vials only while the Pantry is closed, since the open Pantry already lists them, and the vials never cover the pot', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['starter-shelf']);

  expect(report.wideWithPantry).toEqual({ pantryOpen: true, starters: 0, coverPot: false });
  expect(report.compactWithPantry).toEqual({ pantryOpen: true, starters: 0, coverPot: false });
  expect(report.wideWithPotions).toEqual({ pantryOpen: false, starters: 6, coverPot: false });
  expect(report.compactClosed).toEqual({ pantryOpen: false, starters: 6, coverPot: false });
});
test('Wire records survive the shelf: setting a step aside and pouring it back keeps every original wire id in its original place, in one session and after Save and reopen', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-wire-records-'));
  try {
    const report = await studioProbe(['wire-records', join(dir, 'studio.sqlite')]);

    expect(report).toEqual({ sameSession: true, saved: true, afterReopen: true, missing: [] });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test('Grimoire room: the default Support and Parallel Research potions leave a wide channel between every wired pair of cards so no wire loops back behind a card, and twin names keep their number when a title is shortened', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['grimoire-room']);

  for (const [potion, room] of Object.entries<any>(report)) {
    expect({ potion, tight: room.tight }).toEqual({ potion, tight: [] });
    expect({ potion, twinsCut: room.twinsCut }).toEqual({ potion, twinsCut: [] });
  }
});
test('Log counts: a brew where one template feeds eight bottles counts each step once in the Cauldron Log, matching the ten steps on the cauldron', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['log-counts']);

  expect(report).toEqual({ distinct: 10, saysSteps: true });
});

test('Pantry steps that need a list can be set up: a Switch takes its cases as plain words, grows one output per case before default, routes a matching message down its case and anything else to default, keeps its cases after reopening, drops an unwired case when it is removed, and Destructure JSON grows one output per key', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['pantry-settings']);

  expect(report.fieldShown).toBe(true);
  expect(report.typedOutputs).toEqual(['refund', 'billing', 'default']);
  expect(report.saved).toEqual(['refund', 'billing']);
  expect(report.check).toBe('valid');
  expect(report.refundReply).toBe('Refunds desk');
  expect(report.otherReply).toBe('Front desk');
  expect(report.reopenedOutputs).toEqual(['refund', 'billing', 'default']);
  expect(report.reopenedText).toBe('refund, billing');
  expect(report.trimmedOutputs).toEqual(['refund', 'default']);
  expect(report.keysField).toBe(true);
  expect(report.keyOutputs).toEqual(['name', 'age']);
  expect(report.keyConfig).toEqual(['name', 'age']);
});

test('An empty pot shows its recipe card in the Recipe tab, naming the potion, counting nothing yet and listing the empty pot and bottle sections, instead of a lone heading over blank space', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['empty-recipe']);

  expect(report.placeholder).toBe(false);
  expect(report.sections).toBe(true);
  expect(report.texts).toEqual(expect.arrayContaining(['Fresh Potion', '0 ingredients · 0 bottles', 'In the pot', 'Bottles']));
  expect(report.texts.filter((line: string) => line === 'None yet')).toHaveLength(2);
});

test('A published potion keeps saying which version is live in the Potions list after another potion is opened', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-seals-'));
  try {
    const report = await studioProbe(['shelf-seals', join(dir, 'studio.sqlite')]);

    expect(report.openSpell).toBe('Research Agent');
    expect(report.publishedRow).toContain('v1 live');
    expect(report.draftRow).not.toContain('live');
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('List settings stay true: an imported case with a comma survives Check and shows quoted, the boxes are labelled Cases, Outputs and Keys, an empty case list is caught by Check, a Split shows only the outputs it lists, and a case still wired after it is removed is named by Check', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['list-truth']);

  expect(report.kept).toEqual({ check: 'valid', cases: ['north,west', 'south'], shown: '"north,west", south', outputs: ['north,west', 'south', 'default'] });
  expect(report.captions).toEqual({ switch: 'Cases', split: 'Outputs', pick: 'Keys' });
  expect(report.blank.check).toBe('invalid');
  expect(report.blank.errors.join(' ')).toMatch(/needs cases/);
  expect(report.split).toEqual({ default: 'out_0, out_1', outputs: ['left', 'right'] });
  expect(report.stale.check).toBe('invalid');
  expect(report.stale.errors.join(' ')).toMatch(/billing/);
  expect(report.stale.outputs).toEqual(['refund', 'billing', 'default']);
});

test('Check and Save tell the truth: a draft holding only a variable saves from the toolbar and asks before you leave once it is emptied again, Check reads placeholders the way a brew does, and Check with no edits keeps a failed brew on screen', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-check-save-'));
  try {
    const report = await studioProbe(['check-save-truth', join(dir, 'studio.sqlite')]);

    expect(report.draft).toEqual({ saved: true, askedAfterEmptying: true });
    expect(report.literal).toEqual({ check: 'valid', errors: [] });
    expect(report.failedBrew).toEqual({ before: 'Some(Failed)', after: 'Some(Failed)', failedCards: 1, stale: false });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('One action is one undo: a Send Message tossed into an empty pot and set aside on its own, a duplicated step with its settings, and a step poured back from the shelf each go away with a single Cmd or Ctrl Z', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['one-step-undo']);

  expect(report.toss).toEqual({ added: 1, entries: 1, afterUndo: 0 });
  expect(report.duplicate).toEqual({ after: 4, entries: 1, afterUndo: 3 });
  expect(report.pour).toEqual({ poured: true, shelvedAfterUndo: true });
});

test('Wires come home in their own place: setting aside two steps and pouring them back in turn leaves every wire with its original id and order, and undoing a set-aside does the same', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['wire-return']);

  expect(report.poured).toEqual(report.original);
  expect(report.pouredMatches).toBe(true);
  expect(report.undoMatches).toBe(true);
});

test('Every Pantry name shows whole at 960 and 1280: a long name wraps onto a second line instead of ending in an ellipsis', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['pantry-whole']);

  for (const width of ['960', '1280']) {
    expect(report[width].seen).toBeGreaterThan(30);
    expect({ width, cut: report[width].cut }).toEqual({ width, cut: [] });
  }
});

test('An empty pot offers only things you can press: no lone heading, the triggers glow in the Pantry when it is open and stand beside the pot as starters when it is not, never both', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['empty-start']);

  expect(report['wide-pantry']).toEqual({ inertHeading: false, offered: 6, glowing: 6, starters: 0, pantryRows: 6 });
  expect(report['wide-potions']).toEqual({ inertHeading: false, offered: 6, glowing: 0, starters: 6, pantryRows: 0 });
  expect(report.compact).toEqual({ inertHeading: false, offered: 6, glowing: 0, starters: 6, pantryRows: 0 });
});

test('Potions flow downhill: in every everyday layout at 1280 and 960 no bottle neck sits above the cauldron tap, two or three bottles read in recipe order left to right then top to bottom, and no ingredient kind line is cut', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['downhill']);

  expect(report.uphill).toEqual([]);
  expect(report.misordered).toEqual([]);
  expect(report.cutKinds).toEqual([]);
});

test('A failed pour aims at the pot: the broken stub of a failed step runs on until it dives toward the cauldron or reaches its rim, and its cross never lands on a neighbouring live pour, at 960 and 1280', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['failed-stub']);

  expect(report.upward).toEqual([]);
  expect(report.crowding).toEqual([]);
  expect(report.hanging).toEqual([]);
});

test('Variables can drive step settings: a setting written as {{name}} brews with that variable, Check names one that no variable fills, the Support sample binds Search Memory and Generate Text to its variables so none reads Unused, and the temperature dial on a bound step edits the variable', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['bound-settings']);

  expect(report.quick).toEqual({ check: 'valid', outcome: 'Succeeded', waitedForVariable: true });
  expect(report.unknown.check).toBe('invalid');
  expect(report.unknown.errors.join(' ')).toMatch(/no variable is named nope/);
  expect(report.hints).toEqual([['max_memories', '→ Search Memory'], ['temperature', '→ Generate Text']]);
  expect(report.dial).toEqual({ shown: 0.4, variable: 'Float(0.5)', config: '{{temperature}}' });
});

test('Two bottles fed by the same route get their own numbered names while a lone route keeps its plain name', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['twin-routes']);

  expect(report.names).toEqual(['Send · billing 1', 'Send · billing 2', 'Send · default']);
});

test('Small things stay honest: the Pantry search Clear button is at least 24 by 24, a Switch case that reuses the name default is named by Check, and undoing an older add keeps a card you moved since where you put it', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['small-truths']);

  expect(report.clear[0]).toBeGreaterThanOrEqual(24);
  expect(report.clear[1]).toBeGreaterThanOrEqual(24);
  expect(report.clash.check).toBe('invalid');
  expect(report.clash.errors.join(' ')).toMatch(/default is already an output/);
  expect(report.undoKeepsMove).toEqual({ added: true, addUndone: true, replyStays: true });
});

test('Cauldron cards say what they are set to instead of their code name: Generate Text shows its model, Search Memory how many it recalls, a Switch how many cases it routes, while the Grimoire keeps its own headers', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['card-notes']);

  expect(report.llm).toContain('gpt-4o-mini');
  expect(report.llm).not.toContain('ai/generate_text');
  expect(report.recall).toContain('top 5 · max_memories');
  expect(report.switch).toContain('2 cases');
  expect(report.grimoireShowsNote).toBe(false);
});

test('Variable edits undo and rename cleanly: a bound temperature press, a typed default and a typed rename each undo with one Cmd or Ctrl Z, and a rename carries every {{name}} that uses it so the step stays bound and Check stays clean', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['var-undo']);

  expect(report.dial).toEqual({ after: 'Float(0.5)', undone: 'Float(0.4)' });
  expect(report.typed).toEqual({ after: 'Int(12)', undone: 'Int(5)' });
  expect(report.rename.after).toEqual({ name: 'recall_count', limit: '{{recall_count}}', hint: '→ Search Memory' });
  expect(report.rename.check).toBe('valid');
  expect(report.rename.undone).toEqual({ name: 'max_memories', limit: '{{max_memories}}' });
});

test('Bound settings say where they come from: the temperature tile shows its bound look without repeating a variable named like the field, a card bound to a variable shows its value and the variable, and an open published potion says Live once with its version on the pill', { timeout: 900_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'magicklala-studio-bound-looks-'));
  try {
    const report = await studioProbe(['bound-looks', join(dir, 'studio.sqlite')]);

    expect(report.tile).toContain('Temperature');
    expect(report.tile).not.toContain('Temperature · temperature');
    expect(report.tileCut).toEqual([]);
    expect(report.recall).toBe('top 5 · max_memories');
    expect(report.activeRow).toContain('v1 Live');
    expect(report.activeRow.filter((line: string) => /live/i.test(line))).toEqual(['v1 Live']);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('Redo brings back what undo took: Cmd or Ctrl Shift Z re-applies an undone removal, undo takes it back again, and a new edit after undo clears redo so it never overwrites fresh work', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['redo-order']);

  expect(report).toEqual({ removed: true, undone: true, redone: true, undoneAgain: true, freshKept: 'Fresh: {{content}}', echoAfterStaleRedo: true });
});

test('Parked wires survive undo and reload: undoing a pour-back puts the step back on the shelf with all four parked wires so pouring again rebuilds the original recipe, and a potion saved with two steps aside reopens and pours back in the original wire order', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['aside-undo-pour']);

  expect(report.parkedAside).toBe(4);
  expect(report.pouredMatches).toBe(true);
  expect(report.undone).toEqual({ shelved: true, parked: 4 });
  expect(report.repoured).toEqual(report.original);
  expect(report.reloaded).toEqual(report.twoOriginal);
});

test('Each picker click and dial press is its own undo step: two model picks undo one at a time back to the original model, and two presses of the bound temperature dial undo one press at a time', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['picker-undo']);

  expect(report.models).toEqual(['gpt-4.1', 'gpt-4o', report.original]);
  expect(report.temperatures).toEqual(['Float(0.6)', 'Float(0.5)', 'Float(0.4)']);
});

test('A Split whose Outputs are cleared stays cleared: the saved list is empty instead of falling back to out_0 and out_1, and Check flags both the empty list and the wire still leaving the old left output', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['split-cleared']);

  expect(report.before).toBe('valid');
  expect(report.outputs).toEqual([]);
  expect(report.after).toBe('invalid');
  expect(report.errors).toContain('Split: needs outputs');
  expect(report.errors).toContain('Split: left is no longer listed but still has a wire');
});

test('One brew has one set of step numbers: the canvas badges and the Trace list agree for a subgraph call and a failed step, child steps nest under their call as 2.1 and 2.2, the header counts top-level steps, and it names a cancelled or timed-out brew as such', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['step-numbers']);
  const steps = ['1 On Message 1ms Inspect ›', '2 Subgraph 2ms Inspect ›', '2.1 call › inner 1ms Inspect ›', '2.2 call › inner_two 1ms Inspect ›', '3 Rhai Script boom Inspect ›'];

  for (const [status, word] of [['failed', 'Failed'], ['cancelled', 'Cancelled'], ['timed_out', 'Timed out']]) {
    expect(report[status].badges).toEqual({ trigger: 1, call: 2, script: 3, reply: null });
    expect(report[status].steps).toEqual(steps);
    expect(report[status].log).toBe('1 On Message · 1ms 2 Subgraph · 2ms 3 Rhai Script: boom');
    expect(report[status].inspectedChild).toBe('call');
    expect(report[status].header).toBe(`Brew #1 · ${word} 3 steps · 7 events · <1ms total`);
  }
});

test('A newer saved draft never hides the version that is still live: after publishing v1 and saving a different draft, the open potion row says Draft with v1 live, and it still says v1 live after opening another potion', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['live-after-draft']);

  expect(report.openRow).toBe('Tasting Agent 2 ingredients · v1 live Draft');
  expect(report.elsewhereRow).toBe('Tasting Agent 2 ingredients · v1 live Open ›');
});

test('Check holds a bound setting to the same rules as a typed one: a temperature variable of 5 and a memory limit variable of 500 are flagged as out of range, and a temperature variable that holds a word is flagged as not a number', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['bound-checks']);

  expect(report.clean).toBe('valid');
  expect(report.ranged).toEqual([
    'Search Memory: limit needs a whole number from 1 to 100, but the variable max_memories is 500',
    'Generate Text: temperature needs a number from 0 to 2, but the variable temperature is 5',
  ]);
  expect(report.worded).toEqual(['Generate Text: temperature needs a number from 0 to 2, but the variable temperature holds text']);
});

test('A step fresh from the Pantry shows the settings it will really use: model steps name the default model, Delay shows 1000 ms, Split counts its two default outputs, searches show top 5, and a Switch with no cases shows no note', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['spawned-notes']);

  expect(report).toEqual({
    'ai/generate_text': 'gpt-4o-mini',
    'ai/stream_text': 'gpt-4o-mini',
    'ai/structured_json': 'gpt-4o-mini',
    'memory/search': 'top 5',
    'knowledge/search': 'top 5',
    'time/delay': '1000 ms',
    'flow/split': '2 outputs',
    'flow/switch': null,
  });
});

test('Filtering the Pantry never cuts a name: with "on" or "var" typed at both sizes every matching row shows its whole name as one wrapped title with the match picked out, and no row ends in an ellipsis', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['pantry-filtered']);

  for (const [key, least] of [['960-on', 8], ['1280-on', 8], ['960-var', 4], ['1280-var', 4]] as const) {
    expect(report[key].seen).toBeGreaterThanOrEqual(least);
    expect({ key, cut: report[key].cut }).toEqual({ key, cut: [] });
  }
});

test('A lone bottle tag sits centred until it has a step to show: before a brew the tag keeps its plain 10 point inset with no badge, and a bottle that fails during a brew wears its step number', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['stand-badge']);

  expect(report.idleBadge).toBe(false);
  expect(report.idleInset).toBe(10);
  expect(report.failedStep).toBe(5);
  expect(report.failedBadge).toBe('5');
});

test('Rack glass carries the potion the whole way: every stream runs inside the glass from tap to bottle neck, no glass runs on dry past the last bottle, and an untaken route starts its dashes only where it leaves the brewed flow', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['rack-pipes']);

  for (const key of ['router@1280', 'eight@1280', 'eight@960']) {
    expect(report[key].glass).toBeGreaterThan(0);
    expect({ key, leaks: report[key].leaks, dry: report[key].dry, overGold: report[key].overGold }).toEqual({ key, leaks: 0, dry: 0, overGold: 0 });
  }
});

test('Long bottle names never break the rack: three long route names wrap whole and every tag stays on the canvas, and in an eight-bottle rack every tag keeps the part of its name that differs instead of cutting it off', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['long-routes']);

  for (const key of ['three@1280', 'three@960', 'eight@1280', 'eight@960']) {
    expect({ key, hidden: report[key].hidden, belowCanvas: report[key].belowCanvas }).toEqual({ key, hidden: [], belowCanvas: [] });
  }
});

test('A card menu never floats over the pour fan or the Cauldron and Grimoire switch at either size: it either finds clear canvas or docks into the Recipe tab, opening that tab and sitting inside its scroll, and it does not follow you onto Variables', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['menu-fan']);

  expect(report.floatingOnFan).toEqual([]);
  expect(report.onSwitcher).toEqual([]);
  expect(report.docked).toBeGreaterThan(0);
  expect(report.dockedOffRecipe).toEqual([]);
  expect(report.dockedOutsideScroll).toEqual([]);
  expect(report.dockedRepeatsTitle).toEqual([]);
});

test('Bottles never share a visible name: two Sends fed by an orders route on two different Switches read Send · orders 1 and Send · orders 2, while a unique route keeps its plain name', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['twin-switch-names']);

  expect(report.titles).toEqual(['Send · orders 1', 'Send · refunds', 'Send · orders 2']);
});

test('Redo never crosses potions: after undoing an edit in one potion and opening another, there is nothing to redo and redo leaves the newly opened recipe untouched', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['redo-across']);

  expect(report).toEqual({ name: 'Research Agent', redoWaiting: false, unchanged: true });
});

test('Publishing ships every trigger that was not set aside: a recipe with a wired On Start and a standalone On Webhook releases both, so which trigger you test with never changes what ships', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['lone-trigger-ships']);

  expect([...report.shipped].sort()).toEqual(['hook', 'say', 'start']);
});

test('A variable rename only lands on a valid unused name and carries every use: {{ max_memories.value }} follows the rename with its path, and typing through another variable name keeps the old name and flags Check until the name is unique, so neither variable steals the other one uses', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['rename-safe']);

  expect(report.pathedPrompt).toBe('Use {{ recall.value }} and {{recall}}');
  expect(report.clashCheck).toBe('invalid');
  expect(report.namesMid).toEqual(['max_memories', 'temperature']);
  expect(report.namesAfter).toEqual(['temperaturex', 'temperature']);
  expect(report.limit).toBe('{{temperaturex}}');
  expect(report.temperature).toBe('{{temperature}}');
  expect(report.spaced).toEqual({ check: ['max memories: a variable name uses only letters, digits, _ and -'], name: 'max_memories' });
});

test('A Split fresh from the Pantry shows the two outlets it will really fire, out_0 and out_1, until you change or clear its list', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['fresh-split']);

  expect(report.outlets).toEqual(['out_0', 'out_1']);
});

test('Check judges bound settings the way the brew does: a template that is just {{count}} prints the number and passes, while a loop limit variable of 1.5 is flagged because the loop needs a whole number', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['check-matches-brew']);

  expect(report.printed).toEqual({ check: 'valid', errors: [] });
  expect(report.halfLoop).toContain('While Loop: max iterations needs a whole number of at least 1, but the variable limit is 1.5');
});

test('A crowded rack of broken bottles stays readable: with fourteen Sends missing their content every bottle wears its own error mark inside its tag, no mark touches another bottle or mark at either size, and the selected bottle explains the problem in Recipe', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['rack-errors']);

  for (const width of ['960', '1280']) {
    expect({ width, ...report[width] }).toEqual({ width, check: 'invalid', unmarked: [], clashes: [], recipeTells: true });
  }
});

test('Untaken means skipped by a finished brew: while the first Delay is still running the steps after it never say Untaken, and a Quenched brew leaves no step marked Untaken either, while every step a real finished brew skipped does say Untaken', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['waiting-not-untaken']);

  expect(report.whileBrewing).toEqual([]);
  expect(report.afterQuench).toEqual([]);
  expect(report.finishedSkipped.idle).toBeGreaterThan(0);
  expect(report.finishedSkipped.marked).toBe(report.finishedSkipped.idle);
});

test('Selecting a vial in a crowded kitchen lights its whole pour: its strand is drawn lit and on top of the bundle all the way to the rim, and no other strand is lit', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['lit-strand']);

  expect(report.drawnLast).toBe(report.picked);
  expect(report.lit).toEqual([report.picked]);
});

test('Picking Gemini in the model menu saves a provider the step accepts: gemini-2.5-pro is stored with provider gemini, one of the provider choices the Generate Text step declares', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['gemini-pick']);

  expect(report.model).toBe('gemini-2.5-pro');
  expect(report.provider).toBe('gemini');
  expect(report.allowed).toContain(report.provider);
});

test('Undo after redo walks straight back: two model picks, undo, redo, then two undos land on gpt-4o and then the original model with no dead press, and the step you were editing stays selected throughout', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['redo-then-undo']);

  expect(report.seen).toEqual(['gpt-4o', 'gpt-4.1', 'gpt-4o', report.original]);
  expect(report.selected).toBe('llm');
});

test('Check judges a bound loop limit even without a value to read: a Float variable holding 3.0 passes, an Int variable with no value and a true-or-false variable are both flagged before the brew can fail', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['bound-unset']);

  expect(report.whole).toEqual([]);
  expect(report.unset).toEqual(['While Loop: max iterations needs a whole number of at least 1, but the variable limit has no value']);
  expect(report.flag).toEqual(['While Loop: max iterations needs a whole number of at least 1, but the variable limit holds true or false']);
});

test('A step that finishes a pass and then fails shows its failure in the log: a While Loop that hits its limit reads as a red failed step 2 with the limit message, not as a green finish, and the expanded log ends on Failed rather than Brewed', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['finish-then-fail-log']);

  expect(report.log).toBe('1 On Message · 1ms 2 While Loop: while loop exceeded max_iterations 1');
  expect(report.finalRow).toMatch(/Failed/);
  expect(report.finalRow).not.toMatch(/Brewed/);
});

test('A step that was only cut short is Stopped, not Failed: when one branch fails, the Delay still running on the other branch turns grey and reads Stopped with no error mark, while the step that failed stays red with its number', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['stopped-not-failed']);

  expect(report.broken).toBe('failed');
  expect(report.brokenWords).toContain('!');
  expect(report.slow).toBe('idle');
  expect(report.slowWords).toContain('Stopped');
  expect(report.slowWords).not.toContain('!');
});

test('Every red tag names what is missing: after Check on the dozen-ingredient potion each flagged card at both sizes shows the name of the input it lacks, never a bare Missing', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['named-missing']);

  expect(report['1280'].vague).toEqual([]);
  expect(report['960'].vague).toEqual([]);
  expect(report['1280'].bare).toEqual([]);
  expect(report['960'].bare).toEqual([]);
});

test('A failed pour stub keeps clear of its neighbours: with Search Memory missing its query the red stub follows its own pour toward the rim and its cross stays at least 10 points from every other stream at both sizes', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['stub-clear']);

  for (const width of ['1280', '960']) {
    expect({ width, ...report[width] }).toEqual({ width, crowded: [], hanging: [], hexed: 1 });
  }
});

test('A brewed crowded kitchen stays tidy at both sizes: no time chip covers a bottle name, every bottle step number sits on its own bottle above its tag instead of on the shelf bracket, and no card time chip covers the card icon, the card name or the card above it', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['kitchen-brewed-tidy']);

  for (const width of ['960', '960-pantry', '1280']) {
    expect({ width, ...report[width] }).toEqual({ width, coveredNames: [], looseBadges: [], coveredIcons: [], coveredTitles: [], chipsOnCardsAbove: [] });
  }
});

test('The Model and Temperature tiles line up: at both sizes the temperature dial sits level with the model name, and a temperature bound to a variable named temperature reads Temperature once', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['tile-align']);

  for (const width of ['1280', '960']) {
    expect({ width, offset: report[width].offset <= 1 }).toEqual({ width, offset: true });
    expect(report[width].label).toBe('Temperature');
  }
});

test('Typing a variable rename one key at a time never takes over a step input: renaming lim to query_limit passes through query, yet the Template that reads its wired query input keeps {{query}}, and only the Delay bound to the variable follows the new name', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['rename-keystrokes']);

  expect(report).toEqual({
    name: 'query_limit',
    template: 'Need {{query}} now',
    ms: '{{query_limit}}',
    queryWired: true,
    check: 'valid',
    landedOnInput: { check: ['query: a step input already uses this name'], name: 'query_limit' },
  });
});

test('Big racks keep the potion in view: after a brew every bottle on a rack of four or more wears its step number up by the cork, clear of the fill line, and none of them shows a time chip, at 960 and 1280 alike', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['seal-clear']);

  for (const scene of ['kitchen@960', 'kitchen@1280', 'eight@1280']) {
    expect(report[scene].bottles).toBeGreaterThanOrEqual(8);
    expect({ scene, onFill: report[scene].onFill, timeChips: report[scene].timeChips }).toEqual({ scene, onFill: [], timeChips: 0 });
  }
});

test('Grimoire status tags never touch the card above: after brewing the webhook router every Untaken or time tag at both sizes keeps at least 2 points of air from every other card', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['perched-clear']);

  for (const width of ['1280', '960']) {
    expect(report[width].perched).toBeGreaterThan(3);
    expect({ width, touching: report[width].touching }).toEqual({ width, touching: [] });
  }
});

test('Card icons stay readable on crowded pots: the smallest ingredient icon is at least 14 points across in the crowded kitchen and the dozen-step potion at 960 by 640 and in the Support Agent at 1280', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['icon-size']);

  for (const scene of ['kitchen@960', 'dozen@960', 'support@1280']) {
    expect({ scene, readable: report[scene] >= 14 }).toEqual({ scene, readable: true });
  }
});

test('The live release keeps its Live mark while the draft changes: v1 says Live with no Restore right after publishing, still says Live after an edit and also offers Restore, and once v2 is published v1 offers only Restore while v2 says Live', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['live-mark-kept']);

  expect(report.fresh).toMatch(/Live/);
  expect(report.fresh).not.toMatch(/Restore/);
  expect(report.edited).toMatch(/Live/);
  expect(report.edited).toMatch(/Restore/);
  expect(report.pending).toBe(1);
  expect(report.older).toMatch(/Restore/);
  expect(report.older).not.toMatch(/Live/);
  expect(report.newest).toMatch(/Live/);
  expect(report.newest).not.toMatch(/Restore/);
});

test('A number variable set earlier on the route can be read by a later step: On Start, Set ms, then Delay {{ms}} passes Check even though ms has no starting value, while a Delay that reads ms before any Set, or after a Set on only one arm of a Branch, still says it has no value', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['set-before-read']);

  expect(report.setFirst).toEqual([]);
  expect(report.neverSet).toEqual(['Delay: ms needs a whole number of at least 0, but the variable ms has no value']);
  expect(report.setAfter).toEqual(report.neverSet);
  expect(report.setOnOneArm).toEqual(report.neverSet);
});

test('A long route name fits its Recipe socket row at both window sizes: urgent_customer_reconciliation_notification shows whole, wrapping if it must, and its Flow label stays inside the panel', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['long-route-row']);

  for (const size of ['960', '1280']) {
    expect(report[size].spill).toEqual([]);
    expect(report[size].nameWhole).toBe(true);
  }
});

test('Bottles whose long route names differ only at the start still get different tags: four Send bottles on alpha_, bravo_, charlie_ and delta_customer_reconciliation_notification show four different names at both window sizes', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['twin-tails']);

  for (const size of ['960', '1280']) {
    expect(report[size].tags.every((tag: string) => tag.length > 0)).toBe(true);
    expect(report[size].distinct).toBe(4);
  }
});

test('A lone bottle the brew skipped reads Untaken and stays empty glass: when a Switch sends the message down an unwired route, its only Send bottle shows Untaken, and the release seal fills it only before any brew', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['skipped-solo']);

  expect(report.tag).toMatch(/Untaken/);
  expect(report.sealedBefore).toBe(true);
  expect(report.sealedAfter).toBe(false);
});

test('Two failures fit the collapsed Cauldron Log strip: when both parallel research steps fail with long errors, every chip stays inside the strip before the pinned result at both window sizes instead of being sliced mid-word', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['two-failures-strip']);

  for (const size of ['960', '1280']) {
    expect(report[size].clipped).toEqual([]);
    expect(report[size].shown.join(' ')).toMatch(/Generate Text|Research/);
  }
});

test('A small rack that wraps to two rows keeps its pipes clear of the tags above: the webhook router brewed at 960 with the Pantry open, and at both sizes without it, runs no shelf pipe behind or within a hair of any bottle tag', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['small-rack-rows']);

  for (const [scene, rack] of Object.entries<any>(report)) {
    expect({ scene, bottles: rack.bottles }).toEqual({ scene, bottles: 3 });
    expect({ scene, crossings: rack.crossings }).toEqual({ scene, crossings: [] });
  }
});

test('Shelf planks stay clear of everything on the tags: on racks of three and eight bottles, brewed or not, at both sizes, no tag, tag text, status chip or step seal touches a plank or its iron brackets', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['plank-clear']);

  for (const [scene, rack] of Object.entries<any>(report)) {
    expect({ scene, rack: rack.rack }).toEqual({ scene, rack: true });
    expect({ scene, clashes: rack.clashes }).toEqual({ scene, clashes: [] });
  }
});

test('A crowded stacked pot keeps its rim clear: with eight or more pours on lanes no ladle wedges between the streams and no froth sits over them, no rising bubble starts where a stream lands or rises through a stream on the branching router, and the stream bundle keeps at least 10 points of air under the cards it passes', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['stacked-pot-clear']);

  for (const [scene, pot] of Object.entries<any>(report)) {
    if (pot.crowded) {
      expect({ scene, ladle: pot.ladle, froth: pot.froth }).toEqual({ scene, ladle: false, froth: false });
    }
    expect({ scene, bubblesOnPours: pot.bubblesOnPours }).toEqual({ scene, bubblesOnPours: [] });
    expect({ scene, frothOverStreams: pot.froth && pot.frothOnStreams }).toEqual({ scene, frothOverStreams: false });
    if (pot.stacked && pot.floorGap !== null) expect(pot.floorGap, scene).toBeGreaterThanOrEqual(10);
  }
  expect(Object.values<any>(report).some((pot) => pot.crowded)).toBe(true);
});

test('A lone bottle shows its names whole: the potion name and the bottle name on a single-bottle tag wrap instead of ending in an ellipsis, at 960 with the Pantry open, after Check, and at 1280', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['solo-tag-whole']);

  expect(Object.keys(report).length).toBe(3);
  for (const [scene, tag] of Object.entries<any>(report)) {
    expect({ scene, cut: tag.cut }).toEqual({ scene, cut: [] });
  }
});

test('Recipe plugs a missing input without leaving the Cauldron: a Send Message with nothing in its content offers Plug in, lists only text outputs of steps that run before it (never a sibling branch or a set-aside step), wires the chosen one so Check passes, and Unplug takes it back out, while flow inputs get no plug button', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['recipe-plug']);

  expect(report.flowButton).toBe(false);
  expect(report.offered).toContain('trigger-content');
  expect(report.offered.every((source: string) => source.startsWith('trigger-') && source !== 'trigger-out')).toBe(true);
  expect(report.plugged).toEqual(['trigger.content']);
  expect(report.checkAfterPlug).toBe('valid');
  expect(report.unplugShown).toBe(true);
  expect(report.afterUnplug).toEqual([]);
  expect(report.forkOffered).toContain('trigger-content');
  expect(report.forkOffered.filter((source: string) => source.startsWith('other-') || source.startsWith('spare-'))).toEqual([]);
});

test('Plug in and Unplug are real buttons: on the Recipe rows of a step missing its query and a step whose query is wired, both buttons are at least 24 points wide and tall at 1280 and 960, and Missing reads as plain red text no taller than a line, not a second pill', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['plug-target-size']);

  for (const width of ['1280', '960']) {
    const sizes = report[width];
    expect(sizes.plug[0], width).toBeGreaterThanOrEqual(24);
    expect(sizes.plug[1], width).toBeGreaterThanOrEqual(24);
    expect(sizes.unplug[0], width).toBeGreaterThanOrEqual(24);
    expect(sizes.unplug[1], width).toBeGreaterThanOrEqual(24);
    expect(sizes.missingPill, width).toBe(false);
  }
});

test('Wide letters get the room they need: three Send bottles on WWWW, MMMM and mmmm routes keep their whole names on their tags and never overlap, at 960 with the Pantry open and at 960 and 1280 without it', { timeout: 900_000 }, async () => {
  const report = await studioProbe(['wide-letters']);

  expect(Object.keys(report).length).toBe(3);
  for (const [scene, shelf] of Object.entries<any>(report)) {
    expect({ scene, cut: shelf.cut, overlaps: shelf.overlaps }).toEqual({ scene, cut: [], overlaps: [] });
  }
});
