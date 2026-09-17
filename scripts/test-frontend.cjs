// Régressions sur les vrais modules TS/TSX, sans navigateur ni dépendance de test.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const root = path.resolve(__dirname, '..');

function loader(overrides = {}) {
  const cache = new Map();
  function load(filename) {
    filename = path.resolve(filename);
    if (cache.has(filename)) return cache.get(filename).exports;
    const module = { exports: {} }; cache.set(filename, module);
    const output = ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX, esModuleInterop: true },
    }).outputText;
    const localRequire = specifier => {
      if (specifier in overrides) return overrides[specifier];
      if (specifier.endsWith('.css')) return {};
      if (!specifier.startsWith('.')) return require(specifier);
      const base = path.resolve(path.dirname(filename), specifier);
      const found = [base, base + '.ts', base + '.tsx'].find(p => fs.existsSync(p) && fs.statSync(p).isFile());
      return load(found);
    };
    new Function('require', 'module', 'exports', output)(localRequire, module, module.exports);
    return module.exports;
  }
  return filename => load(path.join(root, filename));
}

// Petit pilote de hooks pour les courses entre IPC et brouillons de réglages.
// Il ne prétend pas valider le DOM, la mise en page ou le runtime WebView2.
function hooks() {
  const slots = []; let cursor = 0, changed = false, effects = [];
  const api = { ...React,
    useState(initial) {
      const i = cursor++;
      if (!(i in slots)) slots[i] = { value: typeof initial === 'function' ? initial() : initial };
      return [slots[i].value, next => {
        const value = typeof next === 'function' ? next(slots[i].value) : next;
        if (!Object.is(value, slots[i].value)) { slots[i].value = value; changed = true; }
      }];
    },
    useRef(initial) { const i = cursor++; return slots[i] ??= { current: initial }; },
    useEffect(effect, deps) {
      const i = cursor++, prev = slots[i];
      if (!prev || deps.some((v, n) => !Object.is(v, prev.deps[n]))) {
        slots[i] = { deps, cleanup: prev?.cleanup };
        effects.push(() => { slots[i].cleanup?.(); slots[i].cleanup = effect(); });
      }
    },
  };
  return { api, render(component) {
    let tree;
    for (let pass = 0; pass < 20; pass++) {
      changed = false; cursor = 0; tree = component();
      const pending = effects; effects = []; pending.forEach(effect => effect());
      if (!changed) return tree;
    }
    throw new Error('Rendus sans convergence');
  }, dispose() { slots.forEach(s => s.cleanup?.()); } };
}
function find(tree, predicate) {
  if (Array.isArray(tree)) { for (const child of tree) { const found = find(child, predicate); if (found) return found; } }
  else if (tree && typeof tree === 'object' && tree.props) {
    if (predicate(tree)) return tree;
    return find(tree.props.children, predicate);
  }
}
const load = loader();
const usage = load('src/lib/usage.ts');
const configModule = load('src/lib/config.ts');
const MiniHud = load('src/components/MiniHud.tsx').default;
const provider = { id: 'claude', prefix: '$ claude', dataTs: Date.now()/1000, active: true, windows: [{kind:'5h', usedPercent:42, resetsAt:Date.now()/1000+1000}] };

test('HUD vide : une sortie utilisable reste présente', () => {
  let exited = false;
  const tree = MiniHud({ provider: undefined, onExit: () => { exited = true; } });
  assert.equal(tree.type, 'button'); tree.props.onClick(); assert.ok(exited);
  assert.match(renderToStaticMarkup(tree), /Revenir au widget/);
});
test('HUD expiré : le quota mis en cache ne se présente pas comme actuel', () => {
  const tree = MiniHud({ provider: {...provider, note:'token_expired'}, activeCount:2, onExit: () => {} });
  const html = renderToStaticMarkup(tree);
  assert.match(html, /Reconnecter/); assert.doesNotMatch(html, /42%/);
  assert.match(html, /2 sessions actives/);
});
test('les fenêtres de quota inconnues restent accessibles et l’entrée est préservée', () => {
  const windows = [{kind:'other'}, {kind:'weekly'}, {kind:'5h'}];
  assert.deepEqual(usage.orderedWindows(windows).map(w => w.kind), ['5h','weekly','other']);
  assert.equal(windows[0].kind, 'other');
});
test('fusion partielle des sources', () => {
  const before = {...configModule.DEFAULT_CONFIG, providers:{claude:true,codex:false}};
  assert.deepEqual(configModule.mergeConfig(before,{providers:{claude:false}}).providers, {claude:false,codex:false});
});

test('Réglages : un brouillon survit aux événements, sans écraser les changements du tray', async () => {
  const h = hooks(); let live = structuredClone(configModule.DEFAULT_CONFIG), sent;
  const module = loader({ react: h.api,
    '../lib/useLive': { useLive: () => ({value:live, error:null}) },
    '../lib/appearance': { applyAppearance: () => {} },
    '../lib/bridge': { call: async (command, args) => { assert.equal(command,'patch_config'); sent = args.patch; return live = configModule.mergeConfig(live, sent); } },
  })('src/components/Settings.tsx');
  global.window = { setTimeout: () => 0 };
  let tree = h.render(module.default);
  find(tree, n => n.props.label === 'Notifier au reset').props.onChange(true);
  tree = h.render(module.default);
  find(tree, n => n.props['aria-label'] === 'Seuil 1 en pourcentage').props.onChange({target:{value:'60'}});
  live = configModule.mergeConfig(live, {theme:'dark',notificationsPaused:true,providers:{codex:false}});
  tree = h.render(module.default);
  assert.equal(find(tree, n => n.props['aria-label'] === 'Seuil 1 en pourcentage').props.value, '60');
  await find(tree, n => n.props.className === 'set-save').props.onClick();
  assert.deepEqual(sent, {resetNotifications:true,thresholds:[60,85,95]});
  assert.equal(live.theme, 'dark'); assert.ok(live.notificationsPaused); assert.equal(live.providers.codex,false);
  h.dispose(); delete global.window;
});

test('Réglages : un échec de sauvegarde conserve le brouillon et affiche une erreur', async () => {
  const h = hooks();
  const module = loader({ react:h.api,
    '../lib/useLive':{useLive:() => ({value:configModule.DEFAULT_CONFIG,error:null})},
    '../lib/appearance':{applyAppearance:() => {}},
    '../lib/bridge':{call:async () => {throw new Error('disque indisponible');}},
  })('src/components/Settings.tsx');
  let tree = h.render(module.default);
  find(tree,n => n.props.label === 'Animations douces').props.onChange(false);
  tree = h.render(module.default);
  await find(tree,n => n.props.className === 'set-save').props.onClick();
  tree = h.render(module.default);
  assert.equal(find(tree,n => n.props.label === 'Animations douces').props.checked,false);
  assert.match(find(tree,n => n.props.role === 'alert').props.children,/Impossible d’enregistrer/);
  h.dispose();
});

test('useLive : une réponse initiale retardée ne remplace pas un événement plus récent', async () => {
  const h = hooks(); let receive, resolveInitial, unsubscribed = false;
  const {useLive} = loader({react:h.api, './bridge':{
    subscribe: async (_event, callback) => {receive=callback; return () => {unsubscribed=true;};},
    call: () => new Promise(resolve => {resolveInitial=resolve;}),
  }})('src/lib/useLive.ts');
  const component = () => useLive('get_config','config-updated',0);
  h.render(component); await new Promise(setImmediate);
  receive(2); resolveInitial(1); await new Promise(setImmediate);
  assert.equal(h.render(component).value,2);
  h.dispose(); assert.ok(unsubscribed);
});

test('ancienneté : Claude périmé après 10 min, Codex seulement après un reset', () => {
  const now = 1_800_000_000;
  const claude = { id:'claude', dataTs:now - 601, windows:[{kind:'5h', usedPercent:10, resetsAt:now + 3600}] };
  assert.equal(usage.isStale(claude, now), true);
  assert.equal(usage.isStale({...claude, dataTs:now - 300}, now), false);
  // Codex inutilisé depuis un jour : sa valeur reste la bonne tant qu'aucune fenêtre n'a été réinitialisée.
  const codex = { id:'codex', dataTs:now - 86400, windows:[{kind:'weekly', usedPercent:94, resetsAt:now + 3600}] };
  assert.equal(usage.isStale(codex, now), false);
  assert.equal(usage.isStale({...codex, windows:[{kind:'weekly', usedPercent:94, resetsAt:now - 1}]}, now), true);
  const today = Date.now() / 1000;
  assert.equal(usage.providerStatus({...codex, dataTs:today - 86400, windows:[{kind:'weekly', usedPercent:94, resetsAt:today + 3600}]}), null);
});

test('libellé d’ancienneté lisible en minutes, heures puis jours', () => {
  const now = 1_800_000_000;
  assert.equal(usage.staleLabel(now - 12 * 60, now), '# il y a 12 min');
  assert.equal(usage.staleLabel(now - 3 * 3600, now), '# il y a 3 h');
  assert.equal(usage.staleLabel(now - 3 * 86400, now), '# il y a 3 j');
});
