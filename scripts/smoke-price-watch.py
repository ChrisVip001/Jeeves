#!/usr/bin/env python3
"""Upgrade an isolated v14 database and verify archived duties through the Runtime."""
import argparse
import asyncio
import hashlib
import importlib.util
from pathlib import Path
import sqlite3
import tempfile

ROOT = Path(__file__).resolve().parents[1]
MIGRATIONS = ROOT / 'runtime/crates/jeeves-state/migrations'
spec = importlib.util.spec_from_file_location('recovery', ROOT / 'scripts/smoke-recovery.py')
recovery = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recovery)


def seed(path):
    with sqlite3.connect(path) as db:
        db.execute('''CREATE TABLE _sqlx_migrations (
            version BIGINT PRIMARY KEY, description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL)''')
        for migration in sorted(MIGRATIONS.glob('*.sql')):
            version, description = migration.stem.split('_', 1)
            if int(version) > 14:
                break
            data = migration.read_bytes()
            db.executescript(data.decode())
            db.execute('INSERT INTO _sqlx_migrations VALUES (?,?,CURRENT_TIMESTAMP,1,?,0)',
                       (int(version), description.replace('_', ' '), hashlib.sha384(data).digest()))
        db.executescript('''
            INSERT INTO agent_tasks(id,title,session_id,status)
            VALUES('legacy-task','Legacy observation','jeeves-main','running');
            INSERT INTO tool_operations(id,task_id,turn_id,session_id,round_id,tool_use_id,tool,input_json,input_digest)
            VALUES('old-operation','legacy-task','old-turn','jeeves-main','old-round','old-call','task','{}','digest');
            INSERT INTO conversation_handoffs(task_id,session_id,message_id,instruction,context,evidence_json)
            VALUES('legacy-task','jeeves-main','old-message','Watch observation','','{}');
            INSERT INTO price_watches(task_id,operation_id,quote_json,threshold_minor,expires_ms,next_check_ms)
            VALUES('legacy-task','old-operation','{"url":"https://source.example/item","model":"B","spec":"256 GB","currency":"CNY"}',180000,4070908800000,1);
            INSERT INTO watch_observations VALUES('legacy-task',60000,1,180000,'{"totalMinor":190000}');
            INSERT INTO responsibilities(id,purpose,sources_json,cron,expires_ms,budget_json,revision,enabled_ms,clock_owner,plan_json)
            VALUES('legacy-morning','My old schedule','["calendar"]','0 8 * * *',4070908800000,'{"tools":2}',4,1,'morning',
            '{"trigger":{"kind":"cron","expression":"0 8 * * *","timeZone":"Asia/Shanghai"},"action":{"kind":"morningBriefing","version":1,"config":{}},"delivery":{"kind":"draft"}}');
            INSERT INTO responsibilities(id,purpose,sources_json,cron,expires_ms,budget_json,revision,enabled_ms,
                research_task_id,feishu_recipient,last_slot,plan_json)
            VALUES('legacy-weekly','My verified evidence','["https://source.example/evidence"]','0 9 * * 1',
                4070908800000,'{"tools":2}',7,1, 'legacy-task','oc_saved',28928160,
                '{"trigger":{"kind":"cron","expression":"0 9 * * 1","timeZone":"UTC"},"action":{"kind":"researchDigest","version":1,"config":{"taskId":"legacy-task","sources":["https://source.example/evidence"]}},"delivery":{"kind":"feishu","recipient":"oc_saved"}}');
            INSERT INTO receipts(task_id,kind,ref,status) VALUES('legacy-task','observation','old-receipt','confirmed');
        ''')
        for duty, revision in [('legacy-morning', 4), ('watch-legacy-task', 1)]:
            for state, run in [('delivered', 'confirmed'), ('pending', 'sending'), ('pending', None)]:
                key = f'{duty}-{run}'
                db.execute('INSERT INTO wake_inbox(source_key,target_task,state,responsibility_revision) VALUES(?,?,?,?)',
                           (key, f'responsibility:{duty}', state, revision))
                if run:
                    db.execute('INSERT INTO responsibility_runs(wake_key,responsibility_id,status,body) VALUES(?,?,?,?)',
                               (key, duty, run, f'{run} draft'))
        return list(db.execute('SELECT * FROM responsibility_revisions ORDER BY 1,2'))


async def exercise(root, cases):
    home = root / 'home'
    home.mkdir()
    path = home / 'state.db'
    binary = ROOT / 'target/debug/jeeves'
    revisions = seed(path)
    for _ in range(2):
        app = await recovery.App.start(binary, home, root)
        try:
            listed = await app.request('responsibility/list', {})
            duties = {duty['contract']['id']: duty for duty in listed['responsibilities']}
            for name in cases:
                duty = duties[name]
                assert duty['status'] == 'paused' and duty['migrationNote'], duty
                assert duty['revision'] == (5 if name == 'legacy-morning' else 2), duty
                plan = duty['contract'].get('plan')
                if name == 'legacy-morning':
                    assert plan['trigger']['timeZone'] == 'Asia/Shanghai', plan
                    assert plan['action']['config']['sources'] == ['calendar'], plan
                    assert plan['delivery']['kind'] == 'draft', plan
                else:
                    assert plan is None, duty
                result = await app.request('responsibility/reports', {'id': name})
                rows = {row['wakeKey']: row for row in result['reports']}
                assert len(rows) == 3, rows
                assert rows[f'{name}-confirmed']['body'] == 'confirmed draft', rows
                assert rows[f'{name}-confirmed']['status'] == 'confirmed', rows
                assert rows[f'{name}-sending']['body'] == 'sending draft', rows
                assert rows[f'{name}-sending']['status'] == 'review', rows
                assert rows[f'{name}-None']['status'] == 'review', rows
                try:
                    await app.request('responsibility/setPaused', {'id': name, 'paused': False})
                except RuntimeError as error:
                    assert 'confirm a change' in str(error), error
                else:
                    raise AssertionError('Imported schedule resumed without an amended plan')
            with sqlite3.connect(path) as db:
                assert db.execute('SELECT status FROM agent_tasks WHERE id=?', ('legacy-task',)).fetchone() == ('suspended',)
                assert db.execute('SELECT ref,status FROM receipts').fetchall() == [('old-receipt', 'confirmed')]
                assert db.execute('SELECT count(*) FROM watch_observations').fetchone() == (1,)
                existing = db.execute('SELECT * FROM responsibility_revisions ORDER BY 1,2').fetchall()
                assert all(revision in existing for revision in revisions)
                weekly = db.execute("""SELECT status,revision,
                    json_extract(plan_json,'$.action.kind'),json_extract(plan_json,'$.action.config.lookbackDays'),
                    json_extract(plan_json,'$.delivery.kind'),json_extract(plan_json,'$.delivery.connector'),
                    json_extract(plan_json,'$.delivery.recipient')
                    FROM responsibilities WHERE id='legacy-weekly'""").fetchone()
                assert weekly == ('active', 7, 'verifiedSummary', 7, 'connector', 'feishu', 'oc_saved'), weekly
                assert db.execute("SELECT count(*) FROM wake_inbox WHERE state='delivered'").fetchone() == (2,)
        finally:
            await app.kill()
    assert (home / f'state.db.pre-{latest_migration()}').exists()


def latest_migration():
    """The schema version the runtime upgrades to, named by its newest migration file."""
    folder = ROOT / 'runtime/crates/jeeves-state/migrations'
    return max(int(entry.name.split('_', 1)[0]) for entry in folder.glob('[0-9]*_*.sql'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', choices=('morning', 'price', 'all'), default='price')
    choice = parser.parse_args().case
    cases = {'morning': ['legacy-morning'], 'price': ['watch-legacy-task'],
             'all': ['legacy-morning', 'watch-legacy-task']}[choice]
    with tempfile.TemporaryDirectory() as root:
        asyncio.run(exercise(Path(root), cases))
    print(f'PASS recurring migration ({choice}): revision history, draft and receipt preservation, review and restart')


if __name__ == '__main__':
    main()
