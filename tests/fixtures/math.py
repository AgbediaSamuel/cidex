MAX_RECURSION = 1000

def factorial(n):
    if n <= 1:
        return 1
    return n * factorial(n - 1)

# TODO: refactor this to use iteration
def fibonacci(n):
    if n <= 1:
        return n
    return fibonacci(n - 1) + fibonacci(n - 2)
