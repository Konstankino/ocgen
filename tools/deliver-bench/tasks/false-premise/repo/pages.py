"""Addresses of the site's pages."""
from slugs import slugify


def page_url(title):
    return "/pages/" + slugify(title)
