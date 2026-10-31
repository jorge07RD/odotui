from odoo import api, fields, models


class OdotuiNota(models.Model):
    _name = "odotui.nota"
    _description = "Nota de prueba de odotui"

    name = fields.Char(required=True)
    texto = fields.Text()
    largo = fields.Integer(compute="_compute_largo", store=True)
    partner_id = fields.Many2one("res.partner")

    @api.depends("texto")
    def _compute_largo(self):
        for n in self:
            n.largo = len(n.texto or "")
