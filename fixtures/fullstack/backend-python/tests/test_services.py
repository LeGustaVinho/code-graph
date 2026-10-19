from app.services import ItemService


def test_get_returns_item():
    service = ItemService(db=None)
    assert service.get(1).title == "demo"
