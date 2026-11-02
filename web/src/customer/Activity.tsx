/**
 * Everything Gary has done on this server, newest first. Changes are marked apart from
 * reads, because "what did it change?" is the question an owner comes here to answer.
 */
import { Icon } from '../app/icons';
import { useStore } from '../app/store';
import { describeTool, isMutating } from '../gary/derive';
import { Card } from '../ui/ui';

function clock(at: number) {
  return new Date(at).toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' });
}

export function Activity() {
  const { turns, go } = useStore();
  const rows = turns
    .filter((t) => t.steps.length > 0)
    .flatMap((t) => t.steps.map((s, i) => ({ key: `${t.id}-${i}`, at: t.at, prompt: t.prompt, step: s })))
    .reverse();
  const changes = rows.filter((r) => isMutating(r.step)).length;

  return (
    <div className="screen cx-activity">
      <div className="screen-head">
        <h1>Activity</h1>
        <p className="screen-sub">
          {rows.length ? `${rows.length} steps, ${changes} of them changes. Every change was snapshotted first and can be undone.` : 'Nothing yet.'}
        </p>
      </div>
      <Card>
        {rows.length === 0 ? (
          <div className="empty">
            <div className="empty-title">Gary hasn&rsquo;t done anything on this server yet</div>
            <div className="empty-body">
              Ask him something on the{' '}
              <button type="button" className="btn btn-quiet" onClick={() => go('server')}>
                server page
              </button>
            </div>
          </div>
        ) : (
          <ol className="cx-act-list">
            {rows.map((r) => {
              const change = isMutating(r.step);
              return (
                <li key={r.key} className={`cx-act-row ${change ? 'is-change' : ''}`}>
                  <Icon name={change ? 'wrench' : 'check'} size={16} />
                  <div>
                    <div className="cx-act-what">{describeTool(r.step)}</div>
                    <div className="cx-act-why">Asked: &ldquo;{r.prompt}&rdquo;</div>
                  </div>
                  <span className="cx-act-time">{clock(r.at)}</span>
                </li>
              );
            })}
          </ol>
        )}
      </Card>
    </div>
  );
}
