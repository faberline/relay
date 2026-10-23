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

    def test_current_policy_and_missing_blocker_decision_refuse(self):
        with self.assertRaises(RuntimeError):
            policy.check(ROOT / '.github/issue-ci-required-gates.json')
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'policy.json'
            for blockers in (None, 'approved', ['missing coverage']):
                path.write_text(json.dumps(dict(workflows=self.config['workflows'], qualification_blockers=blockers)))
                with self.assertRaises(RuntimeError): policy.check(path)
            path.write_text(json.dumps(dict(workflows=self.config['workflows'], qualification_blockers=[])))
            policy.check(path)
            path.write_text(json.dumps(dict(workflows=[], qualification_blockers=[])))
            with self.assertRaises(RuntimeError): policy.check(path)
