import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).resolve().parent))
import handoff
spec = importlib.util.spec_from_file_location('policy', ROOT / 'scripts/issue-ci/check-policy.py')
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)

class ConsumerPolicyTests(unittest.TestCase):
    def setUp(self):
        self.config = json.loads((ROOT / '.github/issue-ci-required-gates.json').read_text())

    def test_exact_existing_gate_inventory(self):
        expected = [{'path': '.github/workflows/relay-ci.yml', 'jobs': ['fmt, clippy, tests, release contract']}, {'path': '.github/workflows/gke-test.yml', 'jobs': ['build server image / build + publish ghcr test image', 'build test-client image', 'resolve image identity', 'GKE e2e (raft_failover) / deploy + test on shared GKE']}]
        self.assertEqual(self.config['workflows'], expected)
        workflow = (ROOT / '.github/workflows/issue-ci.yml').read_text()
        self.assertIn('workflows: ["relay-ci", "gke-test"]', workflow)
        self.assertLess(workflow.index('python3 scripts/issue-ci/check-policy.py'), workflow.index('python3 scripts/issue-ci/handoff.py'))
        self.assertIn('ref: ${{ github.event.repository.default_branch }}', workflow)
        self.assertNotIn('pull_request_target', workflow)
        self.assertNotIn('download-artifact', workflow)

    def test_every_real_workflow_and_job_is_required(self):
        gates = self.config['workflows']
        snapshots = {}
        for i, gate in enumerate(gates):
            run = dict(id=i, status='completed', conclusion='success', html_url=f'https://github.com/o/r/actions/runs/{i}')
            jobs = [dict(name=name, status='completed', conclusion='success', html_url=f'{run["html_url"]}/job/{j}') for j, name in enumerate(gate['jobs'])]
            snapshots[gate['path']] = run, jobs
        self.assertEqual(handoff.evaluate(gates, snapshots)[0], 'done')
        for gate in gates:
            path = gate['path']
            missing = dict(snapshots); missing.pop(path)
            self.assertEqual(handoff.evaluate(gates, missing)[0], 'pending')
            for index in range(len(gate['jobs'])):
                jobs = list(snapshots[path][1]); jobs.pop(index)
                missing = dict(snapshots); missing[path] = snapshots[path][0], jobs
                self.assertEqual(handoff.evaluate(gates, missing)[0], 'pending')
                for conclusion in ('skipped', 'cancelled', 'neutral'):
                    jobs = [dict(j) for j in snapshots[path][1]]; jobs[index]['conclusion'] = conclusion
                    missing[path] = snapshots[path][0], jobs
                    self.assertEqual(handoff.evaluate(gates, missing)[0], 'pending')

    def test_current_failure_only_policy_is_valid(self):
        self.assertEqual(policy.check(ROOT / '.github/issue-ci-required-gates.json'), self.config)
        self.assertEqual(self.config['lifecycle_mode'], 'failure-only')
        self.assertTrue(self.config['qualification_blockers'])

    def test_guard_refuses_full_or_missing_policy_and_old_controller(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'policy.json'
            for mode in ('full', 'invalid', None):
                config = dict(self.config, lifecycle_mode=mode)
                path.write_text(json.dumps(config))
                with self.assertRaises(RuntimeError): policy.check(path)
            for blockers in (None, 'approved', [], [1], ['']):
                path.write_text(json.dumps(dict(self.config, qualification_blockers=blockers)))
                with self.assertRaises(RuntimeError): policy.check(path)
            config = dict(self.config); config.pop('qualification_blockers')
            path.write_text(json.dumps(config))
            with self.assertRaises(RuntimeError): policy.check(path)
            with patch.object(policy, 'load_controller', return_value=object()):
                with self.assertRaisesRegex(RuntimeError, 'policy-aware controller'):
                    policy.check(path)

    def test_real_gate_failure_only_and_success_never_done(self):
        snapshots = {}
        for i, gate in enumerate(self.config['workflows']):
            run = dict(id=i, status='completed', conclusion='success', html_url=f'https://github.com/o/r/actions/runs/{i}')
            snapshots[gate['path']] = run, [dict(name=name, status='completed', conclusion='success', html_url=run['html_url'] + '/job/1') for name in gate['jobs']]
        self.assertEqual(handoff.evaluate_policy(self.config, snapshots)[0], 'pending')
        path = self.config['workflows'][0]['path']
        snapshots[path][1][0]['conclusion'] = 'failure'
        state, reasons, _ = handoff.evaluate_policy(self.config, snapshots)
        self.assertEqual(state, 'todo')
        self.assertTrue(any(snapshots[path][1][0]['html_url'] in reason for reason in reasons))
