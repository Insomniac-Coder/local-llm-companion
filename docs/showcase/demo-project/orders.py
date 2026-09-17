"""Compute order totals from a JSON order file."""
import json
import sys

TAX_RATE = 0.08


def load_order(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def line_total(item):
    return item["price"] * item["quantity"]


def order_total(items, tax_rate=TAX_RATE):
    subtotal = sum(line_total(item) for item in items)
    return round(subtotal * (1 + tax_rate), 2)


def main(argv):
    if len(argv) != 2:
        print("usage: python orders.py <order.json>")
        return 2
    order = load_order(argv[1])
    print(f"Total: {order_total(order['items']):.2f}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
