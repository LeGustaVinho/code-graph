def outer():
    def target():
        return 1

    def caller():
        return target() + target()

    return caller()
