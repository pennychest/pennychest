from setuptools import setup, find_packages

setup(
    name="pennychest",
    packages=find_packages(),
    package_data={"pennychest": ["seeds/*.json"]},
)
