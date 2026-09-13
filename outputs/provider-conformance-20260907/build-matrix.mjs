import fs from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { Workbook, SpreadsheetFile } from '@oai/artifact-tool';
import { selectFunctionEvidence } from './matrix-evidence.mjs';

const root = path.resolve(import.meta.dirname, '../..');
const out = path.resolve(process.env.WARDIAN_MATRIX_OUTPUT || import.meta.dirname);
const reportDate = process.env.WARDIAN_MATRIX_REPORT_DATE || '2026-09-07';
await fs.mkdir(out, { recursive: true });
const providers = ['claude', 'codex', 'opencode', 'antigravity', 'pi', 'gemini'];
const cases = [
  ['Discovery', 'model_catalog', 'Model catalog refresh', 'CLI model discovery', 'Fresh provider catalog or explicit unavailable result'],
  ['Discovery', 'model_access', 'Direct model access', 'direct CLI', 'Selected model answers an authenticated direct probe; no Wardian runtime claim'],
  ['Discovery', 'model_choice', 'Interactive model choice', 'interactive', 'Observed provider model menu requires action; explicit choice preserves the configured model'],
  ['Lifecycle', 'launch_readiness', 'Launch and readiness', 'interactive', 'Real CLI reaches its ready composer'],
  ['Delivery', 'short_input', 'Short prompt', 'interactive', 'Exact assistant response to short prompt'],
  ['Delivery', 'multiline_input', 'Multiline prompt', 'interactive', 'Assistant responds to complete multiline input'],
  ['Delivery', 'trailing_newline', 'Trailing newline', 'interactive', 'One complete submission with trailing newline'],
  ['Delivery', 'long_input', 'Long pasted prompt', 'interactive', 'One submission preserves independent beginning, middle, and end labels in the native answer'],
  ['Delivery', 'completion_status', 'Completion and idle status', 'interactive', 'Provider completion and Wardian idle agree'],
  ['Delivery', 'delivery_receipt', 'Delivery acknowledgement', 'interactive', 'Wardian acknowledges one submitted turn without a false failure or replay'],
  ['Chat', 'session_identity', 'Provider session identity', 'interactive', 'Verified provider session stored on correct Wardian agent'],
  ['Chat', 'chat_log_link', 'Current chat-log link', 'interactive', 'Link targets current provider log and updates after session change'],
  ['Chat', 'transcript_refresh', 'Live transcript refresh', 'interactive', 'New provider-authored response appears without restart'],
  ['Chat', 'user_provenance', 'Genuine user prompts', 'interactive', 'Real human requests retain native provenance; tool results do not become user prompts'],
  ['Chat', 'context_provenance', 'Injected context roles', 'interactive', 'Observed provider context retains non-user roles and non-request archive provenance'],
  ['Chat', 'request_correlation', 'Request and turn correlation', 'interactive', 'Native requests retain stable request roots and causal references'],
  ['Chat', 'assistant_deduplication', 'Assistant response deduplication', 'interactive', 'One visible answer per provider response'],
  ['Chat', 'tool_calls', 'Tool calls and results', 'interactive', 'Real tool invocation and result rendered with correct identity'],
  ['Chat', 'archive_replay', 'Durable archive replay', 'interactive', 'Archived history preserves roles and content after reload'],
  ['Lifecycle', 'pause_resume', 'Pause and resume', 'interactive', 'Provider resumes exact conversation after pause'],
  ['Lifecycle', 'fresh_session', 'Fresh session', 'interactive', 'New provider identity and no stale chat after fresh restart'],
  ['Lifecycle', 'clear_session', 'Clear session', 'interactive', 'Clear resets active display and correct fresh identity lifecycle'],
  ['Terminal', 'rendering_resize', 'Terminal rendering and resize', 'rendering', 'Real provider output remains readable after resize'],
  ['Terminal', 'scrollback', 'Terminal scrollback', 'rendering', 'Provider-appropriate retained output is readable'],
  ['Telemetry', 'usage_logging', 'Token usage logging', 'interactive', 'Real provider token usage reaches Wardian telemetry'],
  ['Telemetry', 'activity_logging', 'Activity history', 'interactive', 'Real provider or archive-derived activity reaches Wardian history'],
  ['Telemetry', 'cost_logging', 'Cost reporting', 'interactive', 'Cost availability is represented accurately from real source'],
  ['Context', 'instructions', 'Instructions and workspace', 'interactive', 'Provider uses intended workspace and managed instruction marker'],
  ['Context', 'skills', 'Managed skill discovery', 'interactive', 'Assigned skill is discoverable in actual provider session'],
  ['Permissions', 'approvals', 'Approval handling', 'interactive', 'Current real approval prompts enter and leave action-required'],
  ['Permissions', 'automation_approval', 'Automation approval rejection', 'automation', 'Parked approval appears in the public Inbox; rejection prevents provider invocation'],
  ['Headless', 'headless_ephemeral', 'Ephemeral automation', 'headless', 'Temporary provider node completes with assistant output'],
  ['Headless', 'headless_fresh', 'Fresh inherited automation', 'headless', 'Inherited config creates a fresh provider conversation'],
  ['Headless', 'headless_resume', 'Resumed automation', 'headless', 'Exact existing conversation continues through automation'],
  ['Native delivery', 'native_delivery', 'Native broker delivery', 'headless', 'Broker records provider-native turn start and completion'],
  ['Native delivery', 'native_continuity', 'Native session continuity', 'native broker', 'Second provider completion recalls an omitted secret on the same native session binding'],
  ['Native delivery', 'cancellation', 'Cancellation and recovery', 'headless', 'Provider confirms cancellation and a subsequent turn proves execution capacity released; persistent transport may remain'],
  ['Messaging', 'messaging_background_exchange', 'Background task and correlated reply', 'background', 'Canonical task reaches the provider and its exact correlated reply is consumed by the sender'],
  ['Messaging', 'messaging_background_continuity', 'Background conversation continuity', 'background', 'A subsequent task completes on the same conversation across native owner generations'],
  ['Messaging', 'messaging_tui_exchange', 'Original terminal task and reply', 'attached TUI', 'The original local terminal shares the conversation receiving the task and its correlated reply'],
  ['Messaging', 'messaging_non_waking_info', 'Information without a new turn', 'attached TUI', 'Information reaches the existing native conversation without starting a turn'],
  ['Messaging', 'messaging_active_interrupt', 'Active turn interruption', 'attached TUI', 'An explicit interrupt receives the interrupted event for the exact active turn'],
];
const source = 'https://github.com/wardian-app/Wardian/';
const nonCodexMessagingNotes = {
  messaging_background_exchange: 'Generic background task dispatch exists; canonical task, correlated reply and sender consumption still need real-provider acceptance.',
  messaging_background_continuity: 'Generic resume dispatch exists; no qualifying two-task canonical exchange proves retained conversation continuity.',
  messaging_tui_exchange: 'Current non-Codex tasks use idle-only terminal delivery; the original-terminal canonical exchange has no qualifying real-provider assertion.',
  messaging_non_waking_info: 'Current non-Codex information remains available through receive_messages; unsolicited native context injection has no implemented bridge or real-provider acceptance.',
  messaging_active_interrupt: 'Current non-Codex v2 interruption returns unsupported_interrupt because no verified bridge exists. Legacy cancellation is a separate assertion.',
};
const date = '2026-09-07';
const harnessHashes = {};
const headlessEvidenceHash = createHash('sha256').update(await fs.readFile(path.join(root, 'e2e-native/lib/provider-headless-evidence.mjs'))).digest('hex');
for (const [prefix, file] of [['chat/', 'provider-chat-conformance-real-native.test.mjs'], ['context/', 'provider-context-permissions-real-native.test.mjs'], ['native/', 'provider-native-broker-real-native.test.mjs']]) {
  harnessHashes[prefix] = createHash('sha256').update(await fs.readFile(path.join(root, 'e2e-native/tests', file))).digest('hex');
}
let evidence = [];
// Published observations remain the durable baseline when private run inputs
// are absent, or when a later audit contributes only a subset of new runs.
try {
  evidence.push(...(await fs.readFile(path.join(root, 'docs/research/provider-conformance-observations.jsonl'), 'utf8'))
    .split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line)));
} catch (error) { if (error.code !== 'ENOENT') throw error; }
for(const filename of ['coordinator-results.jsonl','provider-conformance-results.jsonl','chat-matrix-results.jsonl','context-matrix-results.jsonl','native-matrix-results.jsonl','rendering-matrix-results.jsonl','codex-messaging-matrix-results.jsonl','resume-matrix-results.jsonl']) {
  try {
    evidence.push(...(await fs.readFile(path.join(root, '.task', filename), 'utf8'))
      .split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line)));
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
}
const rawEvidence=evidence;
let harnessEvidence=[];
evidence=[];
for(const record of rawEvidence) {
  const common={...record,provider_version:record.provider_version||record.version||'',
    wardian_revision:record.wardian_revision||record.build?.match(/\b[0-9a-f]{40}\b/)?.[0]||'',
    date:record.date||date};
  let ids=[];
  if(record.function==='model_probe') {ids=['model_access'];common.mode='direct CLI';}
  else if(record.function==='actual_user_provenance') ids=[record.provider==='pi'?'request_correlation':'user_provenance'];
  else if(record.function==='delivery_and_chat_projection') {
    // These aggregate records prove delivery and refresh. They do not prove
    // exclusion of injected context, physical link correctness or app-restart replay.
    ids=['short_input','multiline_input','trailing_newline','long_input','transcript_refresh','assistant_deduplication','fresh_session'];
  } else if(record.function==='delivery') {
    ids=record.case?.includes('all-cases')?['short_input','multiline_input','trailing_newline','long_input']:['short_input'];
  } else if(record.function==='headless_automation') {ids=['headless_ephemeral'];common.mode='headless';}
  else if(record.function==='rendering') {
    harnessEvidence.push(record);
    if(record.case?.includes('claude')) {ids=['rendering_resize'];common.provider='claude';common.mode='rendering';}
  } else if(cases.some(([,id])=>id===record.function)) ids=[record.function];
  else if(record.function==='all_real_provider_functions') {continue;}
  else if(record.function==='tools_approvals_usage_instructions_native_delivery') {continue;}
  else {harnessEvidence.push(record);}
  for(const id of ids) {
    const normalized={...common,function:id,mode:common.mode||'interactive'};
    if (id === 'long_input' && ['delivery', 'delivery_and_chat_projection'].includes(record.function) && record.status === 'pass') {
      normalized.status='blocked';
      normalized.evidence='Historical suffix-only predicate cannot establish complete long-paste delivery. Superseded by independent beginning, middle, and end labels; current-harness retest required.';
    }
    evidence.push(normalized);
  }
}
// Baseline files are read after coordinator files, so file order is not time
// order. Day-only baseline observations precede timestamped retests that day.
const recordKey = record => JSON.stringify(record, (_key, value) => value && typeof value === 'object' && !Array.isArray(value)
  ? Object.fromEntries(Object.entries(value).sort(([left], [right]) => left.localeCompare(right))) : value);
evidence = [...new Map(evidence.map(record => [recordKey(record), record])).values()];
evidence.sort((a, b) => String(a.date).localeCompare(String(b.date)));
try { harnessEvidence.push(...JSON.parse(await fs.readFile(path.join(out, 'harness-data.json'), 'utf8'))); }
catch (error) { if (error.code !== 'ENOENT') throw error; }
// Merge partial inputs with retained observations, independent of object/file order.
harnessEvidence = [...new Set(harnessEvidence.map(recordKey))].sort().map(value => JSON.parse(value));
for (const record of [...evidence, ...harnessEvidence]) {
  if (/(?:\b[A-Za-z]:[\\/]|[\\/]Users[\\/]|\/home\/)/.test(JSON.stringify(record))) {
    throw new Error('Private path in portable observation record');
  }
}
await fs.writeFile(path.join(root, 'docs/research/provider-conformance-observations.jsonl'),
  evidence.map(record => JSON.stringify(record)).join('\n') + '\n');
await fs.writeFile(path.join(out, 'harness-data.json'), JSON.stringify(harnessEvidence, null, 2));
const labels = {pass:'Pass',fail:'Fail',blocked:'Blocked',untested:'Untested',intentional_skip:'Design skip',reported:'Reported failure'};
let coverage=[];
try {coverage=JSON.parse(await fs.readFile(path.join(root,'.task/harness-coverage.json'),'utf8')).entries;}
catch(error){if(error.code!=='ENOENT')throw error;}
const details = [];
for (const [area, id, title, mode, acceptance] of cases) {
  for (const provider of providers) {
    let status = 'untested';
    const coverageEntry=coverage.find(entry=>entry.function===id);
    let note = coverageEntry?.missing_assertion_reason
      ? `Source audit: ${coverageEntry.missing_assertion_reason} No qualifying run recorded yet.`
      : 'No qualifying real-provider assertion recorded yet.';
    let url = source + 'issues/1159';
    if (provider !== 'codex' && nonCodexMessagingNotes[id]) {
      note = `Source audit only: ${nonCodexMessagingNotes[id]} This is not a real-provider verdict or a permanent design exclusion.`;
    }
    if (provider === 'gemini') { status = 'intentional_skip'; note = 'Unmaintained provider excluded from maintained-provider acceptance; no functionality claim.'; url = source + 'issues/581'; }
    if (provider === 'antigravity' && ['usage_logging','cost_logging'].includes(id)) {
      status = 'intentional_skip'; note = 'Antigravity usage logging intentionally unsupported; no invented token/cost values.';
      url = source + 'blob/39d996c8/docs/specs/2026-08-13-habitat-telemetry-dashboard.md';
    }
    if ((provider === 'opencode' && id === 'chat_log_link') || (provider === 'claude' && id === 'user_provenance')) {
      status = 'reported'; note = 'User-reported failure; independent reproduction pending.';
    }
    const matches = evidence.filter(e => e.provider === provider && e.function === id);
    const first = matches[0];
    const last = selectFunctionEvidence(matches, record => {
      const prefix = Object.keys(harnessHashes).find(value => record.case?.startsWith(value));
      if (record.status === 'pass' && prefix && record.harness_sha256 !== harnessHashes[prefix]) {
        return { ...record, status: 'untested', failure: undefined,
          evidence: `Historical pass on harness ${record.harness_sha256}; current submitted harness ${harnessHashes[prefix]} has no qualifying rerun for this scenario. The ledger preserves the original observation.` };
      }
      if (record.status === 'pass' && prefix === 'context/' && record.case.startsWith('context/headless_') && record.evidence_reader_sha256 !== headlessEvidenceHash) {
        return { ...record, status: 'untested', failure: undefined,
          evidence: 'Historical headless observation does not bind the current native-evidence reader. A qualifying real rerun must record both suite and reader hashes; the ledger preserves the original result.' };
      }
      return record;
    });
    // A no-assertion run cannot revoke an explicit support-policy exclusion.
    // Keep the original observation in the ledger without inventing a result.
    if (last && !(status === 'intentional_skip' && last.status === 'untested')) {
      if (!labels[last.status]) throw new Error(`Unknown result status: ${last.status}`);
      status = last.status; note = last.failure || last.evidence || note;
      url = last.source_url || url;
    }
    details.push({provider,area,function:id,title,mode:last?.mode||mode,baseline:first?labels[first.status]:labels[status],status:labels[status],model:last?.model||'',version:last?.provider_version||'',revision:last?.wardian_revision||'',build:last?.build||'',date:last?.date||'',case:last?.case||'',acceptance,evidence:String(note),source_url:url});
  }
}
const matrixHeaders = ['Area','Function','Claude','Codex','OpenCode','Antigravity','Pi','Gemini','Acceptance'];
const matrixRows = cases.map(([area,id,title,,acceptance])=>[area,title,...providers.map(p=>details.find(d=>d.provider===p&&d.function===id).status),acceptance]);
const detailHeaders = ['Provider','Area','Function','Mode','Baseline','Latest','Model','CLI version','Revision','Build','Observed date','Case','Acceptance','Evidence','Source'];
const detailRows = details.map(d=>[d.provider,d.area,d.title,d.mode,d.baseline,d.status,d.model,d.version,d.revision,d.build,d.date,d.case,d.acceptance,d.evidence,d.source_url]);
for (const row of detailRows) {
  for (const cell of row) {
    if (/(?:\b[A-Za-z]:[\\/]|[\\/]Users[\\/]|\/home\/)/.test(String(cell))) {
      throw new Error('Evidence contains an absolute personal path; sanitize the source record before publication.');
    }
  }
}
const csv = rows => rows.map(row=>row.map(v=>'"'+String(v??'').replaceAll('"','""')+'"').join(',')).join('\n')+'\n';
await fs.writeFile(path.join(root,'docs/research/provider-function-matrix.csv'),csv([matrixHeaders,...matrixRows]));
await fs.writeFile(path.join(root,'docs/research/provider-function-evidence.csv'),csv([detailHeaders,...detailRows]));
await fs.writeFile(path.join(out,'matrix-data.json'),JSON.stringify(details,null,2));

const wb = Workbook.create();
const matrix = wb.worksheets.add('Provider matrix');
const evidenceSheet = wb.worksheets.add('Evidence');
const modelSheet = wb.worksheets.add('Models and scope');
const harnessSheet = wb.worksheets.add('Harness observations');
function tableSheet(sheet,title,subtitle,headers,rows,widths,name) {
  for (const cell of [title, subtitle, ...headers, ...rows.flat()]) {
    if (/(?:\b[A-Za-z]:[\\/]|[\\/]Users[\\/]|\/home\/)/.test(String(cell))) {
      throw new Error('Private path in workbook content');
    }
  }
  sheet.showGridLines = false;
  sheet.getRange('A1').values=[[title]];
  sheet.getRange('A1').format.font={name:'Arial',size:16,bold:true};
  sheet.getRange('A2').values=[[subtitle]];
  const range = sheet.getRangeByIndexes(3,0,rows.length+1,headers.length);
  range.values=[headers,...rows];
  range.format.font={name:'Arial',size:10,color:'#20252B'};
  range.format.rowHeight=32;
  range.format.wrapText=true;
  range.format.verticalAlignment='top';
  const header=sheet.getRangeByIndexes(3,0,1,headers.length);
  header.format.fill='#334155';
  header.format.font={name:'Arial',size:10,color:'#FFFFFF',bold:true};
  for(let i=0;i<widths.length;i++) sheet.getRangeByIndexes(3,i,rows.length+1,1).format.columnWidth=widths[i];
  sheet.tables.add(range,true,name);
  sheet.freezePanes.freezeRows(4);
}
tableSheet(matrix,'Provider function matrix',`Windows, ${reportDate}. Each result retains its observed date and tested build.`,matrixHeaders,matrixRows,[15,35,19,19,19,19,19,19,75],'ProviderMatrix');
tableSheet(evidenceSheet,'Provider evidence','Reported failures and untested coverage remain distinct from reproduced failures.',detailHeaders,detailRows,[14,16,34,17,22,22,30,20,20,28,17,28,65,95,65],'ProviderEvidence');
evidenceSheet.getRangeByIndexes(4,0,detailRows.length,detailHeaders.length).format.rowHeight=60;
let selections=[];
let selectionData = null;
try { selectionData=JSON.parse(await fs.readFile(path.join(root,'.task/model-selections.json'),'utf8')); selections=selectionData.selections; }
catch(error) {if(error.code!=='ENOENT')throw error;}
const modelRows=[];
let refreshedModels = null;
if (process.env.WARDIAN_MATRIX_MODEL_SNAPSHOT) {
  refreshedModels = JSON.parse(await fs.readFile(process.env.WARDIAN_MATRIX_MODEL_SNAPSHOT, 'utf8'));
}
let retainedModels = null;
try { retainedModels = JSON.parse(await fs.readFile(path.join(out, 'model-source.json'), 'utf8')); }
catch (error) { if (error.code !== 'ENOENT') throw error; }
const selectionDate = selectionData?.collected_at || selectionData?.generated_on || '';
const retainedDate = retainedModels?.collected_at || retainedModels?.generated_on || '';
for (const p of providers) {
  const refreshed = refreshedModels?.providers?.[p];
  if (refreshed) {
    const selected = refreshed.selection;
    const previous = [...evidence].reverse().find(record => record.provider === p && record.status === 'pass' && record.model);
    const history = previous ? ` Most recent recorded pass: ${previous.model}, ${previous.date}.` : '';
    const basis = selected.basis || selected.cost_basis || '';
    modelRows.push([p, refreshed.selected_binary_version || refreshed.version || '',
      'Refreshed CLI metadata', selected.model,
      `${selected.status}. ${basis}.${history} ${selected.cheapest_claim || ''}`,
      selected.cost_source || source + 'issues/1159']);
    continue;
  }
  const retained = retainedModels?.selections?.find(row => row.provider === p && typeof row.model === 'string');
  const selection=selections.find(s=>s.provider===p);
  if (retained && (!selection || (selectionDate && retainedDate && selectionDate < retainedDate))) {
    modelRows.push([p, retained.version, retained.catalog_source, retained.model, retained.basis, retained.source_url]);
    continue;
  }
  let catalog=null; try {catalog=JSON.parse(await fs.readFile(path.join(root,`.task/${p}-models.json`),'utf8'));}catch(error){if(error.code!=='ENOENT')throw error;}
  const probe=evidence.find(e=>e.provider===p&&e.function==='model_access');
  const actualNote=probe?`Direct probe passed${probe.resolved_model?` (resolved: ${probe.resolved_model})`:''}. `:'';
  const changedChoice=Boolean(probe&&selection&&probe.model!==selection.selected_model);
  const basis=catalog?.fallback_evidence || (changedChoice
    ? `Initial candidate was ${selection.selected_model}; QA tested ${probe.model}. The relative cost/usability of the untested candidate is unresolved. ${catalog?.basis||''}`
    : selection?`${selection.cheapest_basis} ${probe&&p==='opencode'?'Free routes remain subject to provider rate limits and availability changes.':selection.uncertainty}${probe?'':` ${selection.launch_limitation}`}`
      : p==='gemini'?'Unmaintained; excluded by existing support policy.':'Selection and actual usability verification pending.');
  modelRows.push([p,probe?.provider_version||selection?.provider_version||catalog?.version||'',catalog?.source||selection?.catalog_status||'',probe?.model||selection?.selected_model||'',actualNote+basis,selection?.evidence?.pricing_url||source+'issues/1159']);
}
// Serialize the resolved per-provider union, not a partial private input object.
for (const row of modelRows) {
  if (row.some(value => /(?:\b[A-Za-z]:[\\/]|[\\/]Users[\\/]|\/home\/)/.test(String(value)))) {
    throw new Error('Private path in model evidence');
  }
}
const selected = {
  collected_at: [refreshedModels?.collected_at, selectionDate, retainedDate].filter(Boolean).sort().at(-1) || null,
  scope: 'Model candidates retain source metadata; catalog presence does not establish successful access or quota.',
  selections: modelRows.map(([provider, version, catalog_source, model, basis, source_url]) =>
    ({provider, version, catalog_source, model, basis, source_url})),
};
await fs.writeFile(path.join(out,'model-source.json'),JSON.stringify(selected,null,2));

tableSheet(modelSheet,'Models and scope','A catalog entry alone does not prove access, price or successful execution.',['Provider','CLI version','Catalog source','Selected model','Selection basis and limitation','Source'],modelRows,[18,25,23,40,95,65],'ProviderModels');
modelSheet.getRange('A5:F10').format.rowHeight=82;
tableSheet(harnessSheet,'Harness observations','Harness repairs and setup blockers are separate from provider conformance.',
  ['Scope','Check','Phase','Result','Case','Evidence'],
  harnessEvidence.map(e=>[e.provider,e.function,e.phase||'',labels[e.status]||e.status,e.case||'',`${e.evidence||''}${e.failure?` ${e.failure}`:''}`]),
  [16,32,18,20,36,100],'HarnessObservations');
harnessSheet.getRangeByIndexes(4,0,harnessEvidence.length,6).format.rowHeight=72;
const legendRow = 13;
modelSheet.getRange(`A${legendRow}`).write([
 ['Pass','Exact real-provider assertion passed on the recorded artifact.'],
 ['Fail','Observed behavior contradicted the expected contract.'],
 ['Reported failure','User report awaiting independent reproduction.'],
 ['Blocked','External or harness prerequisite prevented a conformance result.'],
 ['Untested','No qualifying assertion has run for this cell.'],
 ['Design skip','Intentional support exclusion with documented reason.'],
]);
modelSheet.getRange(`A${legendRow}:B${legendRow+5}`).format.rowHeight=38;
modelSheet.getRange(`A${legendRow}:B${legendRow+5}`).format.wrapText=true;
modelSheet.getRange(`A${legendRow}:B${legendRow+5}`).format.rowHeight=58;
for (const [sheet,start] of [[matrix,`C5:H${cases.length+4}`],[evidenceSheet,`E5:F${details.length+4}`]]) {
  for(const [value,fill] of [['Pass','#DCFCE7'],['Fail','#FEE2E2'],['Blocked','#FEF3C7'],['Reported failure','#FFEDD5'],['Design skip','#E2E8F0']]) {
    sheet.getRange(start).conditionalFormats.add('cellIs',{operator:'equal',formula:`"${value}"`,format:{fill}});
  }
}
const statusFills = {Pass:'#DCFCE7',Fail:'#FEE2E2',Blocked:'#FEF3C7','Reported failure':'#FFEDD5','Design skip':'#E2E8F0',Untested:'#F8FAFC'};
for (let row = 0; row < matrixRows.length; row++) {
  for (let column = 2; column < 8; column++) {
    matrix.getRangeByIndexes(row + 4, column, 1, 1).format.fill = statusFills[matrixRows[row][column]];
  }
}
wb.recalculate();
console.log((await wb.inspect({kind:'table',range:'Provider matrix!A4:I10',include:'values,formulas',tableMaxRows:7,tableMaxCols:9,maxChars:1800})).ndjson);
console.log((await wb.inspect({kind:'match',searchTerm:'#REF!|#DIV/0!|#VALUE!|#NAME\\?|#N/A|#NUM!|#NULL!|#SPILL!|#CALC!',options:{useRegex:true,maxResults:20},maxChars:1000})).ndjson);
for(const [sheetName,range,filename] of [['Provider matrix','A1:H16','matrix-preview.png'],['Provider matrix','A17:H38','coverage-preview.png'],['Provider matrix',`A39:I${cases.length+4}`,'messaging-preview.png'],['Evidence','A1:F12','evidence-preview.png'],['Models and scope','A1:E19','models-preview.png'],['Harness observations','A1:F10','harness-preview.png']]) {
  const blob=await wb.render({sheetName,range,scale:1,format:'png'});
  await fs.writeFile(path.join(out,filename),new Uint8Array(await blob.arrayBuffer()));
}
await (await SpreadsheetFile.exportXlsx(wb)).save(path.join(out,'provider-function-matrix.xlsx'));
console.log(JSON.stringify({cells:details.length,counts:details.reduce((a,d)=>(a[d.status]=(a[d.status]||0)+1,a),{})}));
