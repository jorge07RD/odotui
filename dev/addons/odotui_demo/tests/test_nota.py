from odoo.tests import TransactionCase


class TestNota(TransactionCase):
    def test_largo(self):
        n = self.env["odotui.nota"].create({"name": "x", "texto": "hola"})
        self.assertEqual(n.largo, 4)
