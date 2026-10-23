"""Affected unit tests: no network, cloud jobs, or full-suite execution."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SKILLS = Path(__file__).resolve().parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ci = load('handoff', SKILLS / 'handoff.py')
GATES = [{'path': '.github/workflows/unit.yml', 'jobs': ['unit']},
         {'path': '.github/workflows/cloud.yml', 'jobs': ['functional', 'e2e', 'performance']}]
SHA = 'a' * 40


def evidence():
    result = {}
    for i, w in enumerate(GATES, 1):
        run = dict(id=i, run_number=i, run_attempt=1, status='completed', conclusion='success', html_url=f'https://github.com/o/r/actions/runs/{i}')
        jobs = [dict(name=n, status='completed', conclusion='success', html_url=f'{run["html_url"]}/job/{j}') for j, n in enumerate(w['jobs'])]
        result[w['path']] = run, jobs
    return result


class GateTests(unittest.TestCase):
    def test_all_workflows_required(self):
        snapshots = evidence()
        self.assertEqual(ci.evaluate(GATES, snapshots)[0], 'done')
        snapshots.pop(GATES[1]['path'])
        self.assertEqual(ci.evaluate(GATES, snapshots)[0], 'pending')

    def test_non_success_is_never_pass(self):
        for conclusion in (None, 'cancelled', 'skipped', 'neutral', 'stale'):
            with self.subTest(conclusion=conclusion):
                snapshots = evidence()
                snapshots[GATES[1]['path']][1][0]['conclusion'] = conclusion
                self.assertEqual(ci.evaluate(GATES, snapshots)[0], 'pending')

    def test_pending_and_missing_required_jobs(self):
        snapshots = evidence()
        snapshots[GATES[1]['path']][1].pop()
        self.assertEqual(ci.evaluate(GATES, snapshots)[0], 'pending')
        snapshots = evidence()
        snapshots[GATES[0]['path']][0]['status'] = 'in_progress'
        self.assertEqual(ci.evaluate(GATES, snapshots)[0], 'pending')

    def test_failure_returns_todo_with_exact_links(self):
        snapshots = evidence()
        job = snapshots[GATES[1]['path']][1][1]
        job['conclusion'] = 'failure'
        state, reasons, _ = ci.evaluate(GATES, snapshots)
        self.assertEqual(state, 'todo')
        self.assertIn('e2e', reasons[0])
        self.assertIn(job['html_url'], reasons[0])

    def test_failed_run_with_successful_subset_not_pass(self):
        snapshots = evidence()
        snapshots[GATES[1]['path']][0]['conclusion'] = 'failure'
        self.assertEqual(ci.evaluate(GATES, snapshots)[0], 'todo')

    def test_config_refuses_empty_gates_and_duplicate_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'gates.json'
            for workflows in ([], [GATES[0], GATES[0]], [dict(path='evil.py', jobs=['run'])], [dict(path=GATES[0]['path'], jobs=[])]):
                path.write_text(json.dumps(dict(workflows=workflows)))
                with self.subTest(workflows=workflows), self.assertRaises(RuntimeError):
                    ci.load_config(path)

    def test_snapshot_filters_foreign_stale_and_selects_latest_attempt(self):
        good = dict(id=3, run_number=2, run_attempt=2, head_sha=SHA, event='pull_request', head_repository={'full_name': 'o/r'}, path=GATES[0]['path'])
        old = dict(good, id=2, run_attempt=1)
        foreign = dict(good, id=99, head_repository={'full_name': 'fork/r'})
        stale = dict(good, id=100, head_sha='b' * 40)
        with patch.object(ci, 'pages', side_effect=[[old, good, foreign, stale], []]) as pages:
            result = ci.snapshot('o/r', SHA, {'workflows': [GATES[0]]})
        self.assertEqual(result[GATES[0]['path']][0]['id'], 3)
        self.assertIn('/runs/3/attempts/2/jobs', pages.call_args_list[1].args[0])


class ApplyTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.config = Path(self.tmp.name) / 'gates.json'
        self.config.write_text(json.dumps({'workflows': GATES}))
        self.pr = dict(state='open', body='Closes #7', head=dict(sha=SHA, ref='issue/7', repo={'full_name': 'o/r'}))
        self.issue = dict(state='open', labels=[{'name': 'doing'}, {'name': 'priority:p1'}])
        self.writes = []
        self.comments = []
        self.pr_reads = 0
        self.move_on = None
        self.trigger = dict(head_sha=SHA, event='pull_request', id=1)

    def api(self, path, method='GET', data=None):
        if method != 'GET':
            self.writes.append((path, method, data))
            if method == 'POST' and path.endswith('/comments'):
                comment = dict(id=len(self.comments) + 1, body=data['body'], user={'login': 'github-actions[bot]'})
                self.comments.append(comment)
                return comment
            if method == 'PATCH' and '/issues/comments/' in path:
                self.comments[int(path.rsplit('/', 1)[1]) - 1]['body'] = data['body']
            if method == 'POST' and path.endswith('/labels'):
                for label in data['labels']:
                    if label not in {l['name'] for l in self.issue['labels']}:
                        self.issue['labels'].append({'name': label})
            if method == 'DELETE' and '/labels/' in path:
                label = path.rsplit('/', 1)[1]
                self.issue['labels'] = [l for l in self.issue['labels'] if l['name'] != label]
            return {}
        if '/pulls/' in path:
            self.pr_reads += 1
            if self.move_on and self.pr_reads >= self.move_on:
                return dict(self.pr, head=dict(self.pr['head'], sha='b' * 40))
            return self.pr
        if '/actions/runs/' in path:
            return self.trigger
        return self.issue

    def invoke(self, snapshots=None, snapshot_sequence=None):
        args = ['handoff.py', '--repo', 'o/r', '--config', str(self.config), '--pr', '8', '--head', SHA, '--run-id', '1', '--apply']
        with patch.object(sys, 'argv', args), patch.object(ci, 'api', side_effect=self.api), patch.object(ci, 'pages', side_effect=lambda *a: self.comments), patch.object(ci, 'snapshot', side_effect=snapshot_sequence, return_value=snapshots or evidence()), contextlib.redirect_stdout(io.StringIO()):
            return ci.main()

    def test_current_head_success_preserves_other_labels(self):
        self.assertEqual(self.invoke(), 0)
        self.assertIn(('repos/o/r/issues/7/labels', 'POST', {'labels': ['done']}), self.writes)
        self.assertIn(('repos/o/r/issues/7/labels/doing', 'DELETE', None), self.writes)
        self.assertFalse(any('/labels/priority' in w[0] for w in self.writes))

    def test_stale_head_before_and_during_evaluation(self):
        for move_on in (1, 2):
            self.move_on, self.pr_reads = move_on, 0
            with self.assertRaises(RuntimeError):
                self.invoke()
            self.assertEqual(self.writes, [])

    def test_head_moved_after_journal_never_labels(self):
        self.move_on = 3
        with self.assertRaises(RuntimeError):
            self.invoke()
        self.assertTrue(self.writes)
        self.assertTrue(all('/comments' in w[0] for w in self.writes))

    def test_old_event_run_no_mutation(self):
        self.trigger['head_sha'] = 'b' * 40
        with self.assertRaises(RuntimeError):
            self.invoke()
        self.assertEqual(self.writes, [])

    def test_duplicate_failure_never_undoes_later_worker_claim(self):
        snapshots = evidence()
        snapshots[GATES[0]['path']][1][0]['conclusion'] = 'failure'
        self.invoke(snapshots)
        self.writes.clear()
        self.issue['labels'] = [{'name': 'doing'}]  # later worker claim
        self.invoke(snapshots)
        self.assertEqual(self.writes, [])

    def test_late_workflow_failure_receipt_never_reclaims_coding(self):
        first = evidence()
        first[GATES[0]['path']][1][0]['conclusion'] = 'failure'
        first.pop(GATES[1]['path'])
        self.invoke(first)
        later = evidence()
        later[GATES[0]['path']][1][0]['conclusion'] = 'failure'
        self.writes.clear()
        self.invoke(later)
        self.assertEqual(self.writes, [])

    def test_additional_failed_workflow_cannot_undo_claimed_coding_epoch(self):
        first = evidence()
        first[GATES[0]['path']][0]['conclusion'] = 'failure'
        first[GATES[0]['path']][1][0]['conclusion'] = 'failure'
        first[GATES[1]['path']][0].update(status='in_progress', conclusion=None)
        self.invoke(first)
        self.assertIn('todo', {l['name'] for l in self.issue['labels']})
        self.issue['labels'] = [{'name': 'doing'}, {'name': 'priority:p1'}]
        later = evidence()
        for path in (GATES[0]['path'], GATES[1]['path']):
            later[path][0]['conclusion'] = 'failure'
            later[path][1][0]['conclusion'] = 'failure'
        # Same head, run IDs and attempts; only B's pending result changed.
        self.writes.clear()
        comments_before = len(self.comments)
        self.invoke(later)
        self.assertEqual(self.writes, [])
        self.assertEqual(len(self.comments), comments_before)
        self.assertEqual({l['name'] for l in self.issue['labels']}, {'doing', 'priority:p1'})

    def test_snapshot_progress_after_intent_is_retryable(self):
        first = evidence()
        first[GATES[0]['path']][1][0]['conclusion'] = 'failure'
        changed = evidence()
        changed[GATES[0]['path']][1][0]['conclusion'] = 'failure'
        changed[GATES[1]['path']][0]['run_attempt'] = 2
        with self.assertRaises(RuntimeError):
            self.invoke(first, snapshot_sequence=[first, changed])
        self.assertIn(':pending -->', self.comments[0]['body'])
        self.assertEqual({l['name'] for l in self.issue['labels']}, {'doing', 'priority:p1'})
        self.invoke(changed)
        self.assertIn('todo', {l['name'] for l in self.issue['labels']})
        self.assertNotIn('doing', {l['name'] for l in self.issue['labels']})
        self.assertIn(':applied -->', self.comments[0]['body'])

    def test_ready_checkpoint_survives_api_failure_and_later_worker_claim(self):
        failed = evidence()
        failed[GATES[0]['path']][1][0]['conclusion'] = 'failure'
        actual = self.api
        def fail_delete(path, method='GET', data=None):
            if method == 'DELETE':
                raise RuntimeError('transient API failure')
            return actual(path, method, data)
        self.api = fail_delete
        with self.assertRaises(RuntimeError):
            self.invoke(failed)
        self.assertIn(':ready -->', self.comments[0]['body'])
        self.assertIn('todo', {l['name'] for l in self.issue['labels']})
        self.api = actual
        self.invoke(failed)
        self.assertIn(':applied -->', self.comments[0]['body'])
        self.issue['labels'] = [{'name': 'doing'}]
        self.writes.clear()
        self.invoke(failed)
        self.assertEqual(self.writes, [])

    def test_ready_receipt_never_readds_label_consumed_by_worker(self):
        failed = evidence()
        failed[GATES[0]['path']][1][0]['conclusion'] = 'failure'
        self.invoke(failed)
        self.comments[0]['body'] = self.comments[0]['body'].replace(':applied -->', ':ready -->')
        self.issue['labels'] = [{'name': 'doing'}]
        self.writes.clear()
        self.invoke(failed)
        self.assertTrue(all('/issues/comments/' in path for path, _, _ in self.writes))
        self.assertEqual({l['name'] for l in self.issue['labels']}, {'doing'})

    def test_new_failed_attempt_after_success_is_not_old_failure_receipt(self):
        failed = evidence()
        failed[GATES[0]['path']][1][0]['conclusion'] = 'failure'
        self.invoke(failed)
        self.invoke(evidence())
        self.assertIn('done', {l['name'] for l in self.issue['labels']})
        again = evidence()
        again[GATES[0]['path']][0]['run_attempt'] = 2
        again[GATES[0]['path']][1][0]['conclusion'] = 'failure'
        self.invoke(again)
        self.assertIn('todo', {l['name'] for l in self.issue['labels']})
        self.assertNotIn('done', {l['name'] for l in self.issue['labels']})

    def test_untrusted_marker_cannot_suppress_handoff(self):
        self.invoke()
        self.comments[0]['user']['login'] = 'contributor'
        self.issue['labels'] = [{'name': 'doing'}]
        self.writes.clear()
        self.invoke()
        self.assertTrue(self.writes)

    def test_pending_run_never_mutates(self):
        snapshots = evidence()
        snapshots[GATES[0]['path']][0]['status'] = 'queued'
        self.invoke(snapshots)
        self.assertEqual(self.writes, [])

    def test_manually_blocked_issue_is_untouched(self):
        self.issue['labels'].append({'name': 'blocked'})
        with self.assertRaises(RuntimeError):
            self.invoke()
        self.assertEqual(self.writes, [])


if __name__ == "__main__":
    unittest.main()
