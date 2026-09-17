import unittest

from orders import line_total, order_total


class OrderTests(unittest.TestCase):
    def test_line_total_multiplies_price_by_quantity(self):
        self.assertEqual(line_total({"name": "pen", "price": 1.5, "quantity": 4}), 6.0)

    def test_order_total_adds_tax(self):
        items = [
            {"name": "pen", "price": 1.5, "quantity": 4},
            {"name": "notebook", "price": 3.0, "quantity": 2},
        ]
        self.assertEqual(order_total(items, tax_rate=0.1), 13.2)


if __name__ == "__main__":
    unittest.main()
