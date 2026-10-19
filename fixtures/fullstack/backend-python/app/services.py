from app.errors import ItemNotFound
from app.models import Item

CACHE = {}


class ItemService:
    def __init__(self, db):
        self.db = db
        self.lookups = 0

    def get(self, item_id: int) -> Item:
        """Returns an item or raises ItemNotFound."""
        self.lookups += 1
        if item_id in CACHE:
            return CACHE[item_id]
        item = self._load(item_id)
        if item is None:
            raise ItemNotFound(f"item {item_id} missing")
        CACHE[item_id] = item
        return item

    def _load(self, item_id: int):
        if item_id < 0 or item_id > 10_000:
            return None
        return Item(id=item_id, title="demo")
