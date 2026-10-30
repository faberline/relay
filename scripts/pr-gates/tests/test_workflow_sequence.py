"""Protect PR unit-before-integration/cloud ordering and non-PR entry points."""
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[3]


def job(source, name):
    match = re.search(rf'^  {re.escape(name)}:\n(.*?)(?=^  [a-z][a-z-]*:\n|\Z)',
                      source, re.MULTILINE | re.DOTALL)
    if not match:
        raise AssertionError(f'missing workflow job: {name}')
    return match.group(1)


class WorkflowSequence(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.unit = (ROOT / '.github/workflows/pr-unit-static.yml').read_text()
        cls.ci = (ROOT / '.github/workflows/relay-ci.yml').read_text()
        cls.cloud = (ROOT / '.github/workflows/gke-test.yml').read_text()

    def test_reusable_unit_workflow_keeps_each_caller_separate(self):
        triggers = self.unit.split('\npermissions:', 1)[0]
        self.assertRegex(triggers, r'(?m)^  pull_request:$')
        self.assertRegex(triggers, r'(?m)^  workflow_call:$')
        self.assertIn('relay-pr-unit-static-${{ github.workflow }}-${{ github.ref }}', self.unit)
        for source in (self.ci, self.cloud):
            prerequisite = job(source, 'unit-static')
            self.assertIn("if: github.event_name == 'pull_request'", prerequisite)
            self.assertIn('uses: ./.github/workflows/pr-unit-static.yml', prerequisite)

    def test_pr_tests_and_cloud_builds_wait_for_unit_success(self):
        condition = "(github.event_name != 'pull_request' || needs.unit-static.result == 'success')"
        for source, names in ((self.ci, ('gates',)), (self.cloud, ('image', 'test-client'))):
            for name in names:
                with self.subTest(job=name):
                    block = job(source, name)
                    self.assertIn('needs: unit-static', block)
                    self.assertIn(condition, block)
                    self.assertIn('!cancelled()', block)
        self.assertIn('needs: image', job(self.cloud, 'resolve'))
        self.assertIn('needs: [resolve, test-client]', job(self.cloud, 'gke-test'))

    def test_existing_non_pr_entry_points_and_test_commands_remain(self):
        self.assertRegex(self.ci.split('\npermissions:', 1)[0], r'(?m)^  push:$')
        self.assertRegex(self.cloud.split('\npermissions:', 1)[0], r'(?m)^  workflow_dispatch:$')
        gates = job(self.ci, 'gates')
        for command in ('cargo fmt --all -- --check',
                        'cargo clippy --locked --workspace --all-targets --all-features -- -D warnings',
                        'cargo test --locked --workspace',
                        'cargo test --locked --workspace --all-features',
                        'scripts/release/verify-release-contract.py --app relay'):
            self.assertIn(command, gates)


if __name__ == '__main__':
    unittest.main()
