"""Every public process operation must advertise a bounded, non-generic contract."""
import unittest

from jsonschema import Draft202012Validator
import fixture


class OperationContractsTests(unittest.TestCase):
    def test_named_operations_reject_absent_required_fields_and_unexpected_results(self):
        for name in fixture.OPERATIONS:
            for direction, schema in [('input', fixture.operation_input_schema(name)), ('output', fixture.operation_output_schema(name))]:
                with self.subTest(operation=name, direction=direction):
                    Draft202012Validator.check_schema(schema)
                    self.assertEqual(schema['type'], 'object')
                    self.assertIs(schema['additionalProperties'], False)
                    self.assertTrue(schema['required'])
                    validator = Draft202012Validator(schema)
                    self.assertFalse(validator.is_valid({}))
                    self.assertTrue(any(error.validator == 'additionalProperties' for error in validator.iter_errors({'unexpected': 'silent helper regression'})))
                    if direction == 'output':
                        self.assertIn('reportText', schema['required'])


if __name__ == '__main__':
    unittest.main()
